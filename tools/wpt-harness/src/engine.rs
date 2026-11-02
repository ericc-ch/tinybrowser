//! One WPT file, mounted in this process and driven on a virtual clock.

use std::sync::Arc;
use std::time::{Duration, Instant};

use renderer::{EmbeddedRenderer, Mount, RendererEvent, ResponseHead, TabError};
use serde_json::Value;
use url::Url;

use crate::services::{BASE, FsServices};
use crate::suite::{Kind, TestCase};

/// Wall-clock time a test may spend before it is called hung. These are the
/// harness budgets: `DEFAULT_TIMEOUT` + `EXTRA_TIMEOUT` (5 s) and
/// `LONG_TIMEOUT` + `EXTRA_TIMEOUT` (7 s).
///
/// One drain gets the whole budget. A shorter slice would interrupt a script
/// that is still making progress: `testharness.js` catches the engine's
/// `interrupted` error, reports the tests that already ran, and marks the rest
/// `NOTRUN` with a harness error — a truncated file that looks like a verdict.
const NORMAL_BUDGET: Duration = Duration::from_secs(5);

/// Long-budget wall clock.
const LONG_BUDGET: Duration = Duration::from_secs(7);

/// The wall-clock budget for one file.
fn budget(case: &TestCase) -> Duration {
    if case.long_timeout {
        LONG_BUDGET
    } else {
        NORMAL_BUDGET
    }
}

/// Reads the reporter's JSON out of the page.
const POLL: &str =
    "String(globalThis.__tinybrowser_result === undefined ? '' : globalThis.__tinybrowser_result)";

/// The verdict for one file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    /// The file's harness completed and every subtest passed.
    Pass,
    /// The file completed with at least one failure.
    Fail,
    /// The runner killed the file at the budget.
    Timeout,
    /// The engine or the runner could not execute the file.
    Error,
    /// The runner does not implement this kind yet.
    Skip,
}

impl Status {
    /// The status spelling a wptreport uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Timeout => "TIMEOUT",
            Self::Error => "ERROR",
            Self::Skip => "SKIP",
        }
    }
}

/// One testharness subtest.
#[derive(Clone, Debug)]
pub struct Subtest {
    /// Subtest name.
    pub name: String,
    /// `PASS`, `FAIL`, `TIMEOUT`, `ERROR`, or `SKIP`.
    pub status: String,
    /// Failure message.
    pub message: Option<String>,
}

/// The result of one file.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// wptreport test identifier, such as `/dom/nodes/Node-nodeName.html`.
    pub id: String,
    /// Which executor produced the result.
    pub kind: Kind,
    /// File-level verdict used by the summary and the baseline diff.
    pub status: Status,
    /// Failure, skip, or timeout reason.
    pub message: Option<String>,
    /// testharness subtests, or one synthetic subtest for crashtests.
    pub subtests: Vec<Subtest>,
    /// Wall time spent in this file.
    pub duration: Duration,
    /// Every file the test read, for the result cache.
    pub inputs: Vec<std::path::PathBuf>,
}

/// One worker's engine, reused across files.
///
/// Building an engine costs ~100 ms (`QuickJS` runtime, DOM, layout), which is
/// far more than most files cost to run. A browser tab navigates from test to
/// test, so mounting the next file on the same engine is the honest model:
/// `mount_frame` installs a fresh realm and document, and the host state that
/// a test may touch (cookies, storage) is cleared before each mount.
pub struct Session {
    root: std::path::PathBuf,
    services: Arc<FsServices>,
    renderer: EmbeddedRenderer,
}

impl Session {
    /// Creates an engine for one worker.
    #[must_use]
    pub fn new(root: &std::path::Path) -> Self {
        let services = Arc::new(FsServices::new(root.to_path_buf()));
        let renderer = EmbeddedRenderer::new(
            Arc::clone(&services) as Arc<dyn renderer::BrowserServices>,
            net::ValidIdentity::default(),
        );
        Self {
            root: root.to_path_buf(),
            services,
            renderer,
        }
    }

    /// Runs one file.
    ///
    /// `virtual_time` is the runtime's clock mode; the caller owns the runtime
    /// and builds it paused or real.
    pub async fn run(&mut self, case: &TestCase, virtual_time: bool) -> Outcome {
        let started = Instant::now();
        let outcome = match case.kind {
            Kind::Testharness => run_testharness(self, case, virtual_time).await,
            Kind::Crashtest => run_crashtest(self, case, virtual_time).await,
            Kind::Reftest => Err(Failure::Skip("reftest comparison is not implemented".into())),
            Kind::Testdriver => Err(Failure::Skip(
                "testdriver requires a WebDriver session".into(),
            )),
        };
        let (status, message, subtests) = match outcome {
            Ok(result) => (result.status, result.message, result.subtests),
            Err(failure) => {
                let (status, message) = failure.into_parts();
                let subtests = vec![Subtest {
                    name: "Test".into(),
                    status: status.as_str().into(),
                    message: message.clone(),
                }];
                (status, message, subtests)
            }
        };
        Outcome {
            id: case.id(),
            kind: case.kind,
            status,
            message,
            subtests,
            duration: started.elapsed(),
            inputs: self.services.take_inputs(),
        }
    }

    /// Mounts one file and settles its first turn, bounded by `deadline`.
    fn mount(&mut self, case: &TestCase, deadline: Instant) -> Result<(), Failure> {
        // Events belong to the file that produced them. A crashtest is judged
        // by whether it survives, and a stale `ScriptFailed` from the previous
        // file must not be read as this one's.
        let _stale = self.renderer.take_events();
        self.services.clear();
        // The document bytes come from `case.source`, but the file on disk is
        // still an input: editing it must invalidate a cached verdict.
        self.services.record_file(self.root.join(&case.relative));
        let url = Url::parse(&format!("{BASE}{}", case.relative))
            .map_err(|error| Failure::Error(format!("bad url: {error}")))?;
        let path = self.root.join(&case.relative);
        let (content_type, headers) = self.services.headers_for(&path);
        let head = ResponseHead::http(200, url, content_type, &headers);
        self.renderer
            .mount_bounded(
                &Mount {
                    head,
                    body: case.source.as_bytes().to_vec(),
                },
                deadline.saturating_duration_since(Instant::now()),
            )
            .map_err(Failure::Mount)
    }
}

/// A file that completed.
struct Completed {
    status: Status,
    message: Option<String>,
    subtests: Vec<Subtest>,
}

/// Why a file did not complete.
enum Failure {
    Mount(TabError),
    Timeout,
    Error(String),
    Skip(String),
}

impl Failure {
    fn into_parts(self) -> (Status, Option<String>) {
        match self {
            Self::Mount(error) => (Status::Error, Some(error.to_string())),
            Self::Timeout => (Status::Timeout, Some("harness budget elapsed".into())),
            Self::Error(message) => (Status::Error, Some(message)),
            Self::Skip(message) => (Status::Skip, Some(message)),
        }
    }
}

async fn run_testharness(
    session: &mut Session,
    case: &TestCase,
    virtual_time: bool,
) -> Result<Completed, Failure> {
    let started = Instant::now();
    let limit = budget(case);
    let deadline = started + limit;
    session.mount(case, deadline)?;
    let renderer = &mut session.renderer;
    loop {
        let _yielded = renderer.drain_until(deadline);
        let result = renderer.eval(POLL).unwrap_or_default();
        if !result.is_empty() {
            return parse_result(&result);
        }
        if started.elapsed() > limit {
            return Err(timeout_failure(renderer));
        }
        let Some(delay) = renderer.time_until_deadline() else {
            // Nothing is ready and nothing is scheduled. The in-memory host
            // has no pending I/O, so this page cannot make progress.
            return Err(timeout_failure(renderer));
        };
        if !delay.is_zero() {
            if virtual_time {
                tokio::time::advance(delay).await;
            } else {
                tokio::time::sleep(delay.min(Duration::from_millis(50))).await;
            }
        }
    }
}

/// A timeout that names a script failure when the page reported one. A
/// testharness file whose script threw before `setup` leaves no timer and no
/// result, so the event is the only evidence of what happened.
fn timeout_failure(renderer: &mut EmbeddedRenderer) -> Failure {
    let script_failed = renderer
        .take_events()
        .unwrap_or_default()
        .iter()
        .any(|(_, event)| matches!(event, RendererEvent::ScriptFailed));
    if script_failed {
        Failure::Error("script failed".into())
    } else {
        Failure::Timeout
    }
}

async fn run_crashtest(
    session: &mut Session,
    case: &TestCase,
    virtual_time: bool,
) -> Result<Completed, Failure> {
    let started = Instant::now();
    let limit = budget(case);
    let deadline = started + limit;
    session.mount(case, deadline)?;
    let renderer = &mut session.renderer;
    loop {
        let _yielded = renderer.drain_until(deadline);
        if started.elapsed() > limit {
            return Err(Failure::Timeout);
        }
        match renderer.time_until_deadline() {
            Some(delay) if !delay.is_zero() => {
                if virtual_time {
                    tokio::time::advance(delay).await;
                } else {
                    tokio::time::sleep(delay.min(Duration::from_millis(50))).await;
                }
            }
            Some(_) => {}
            None => {
                // A crashtest's contract is that nothing kills the process.
                // Its script is allowed to throw; upstream wptrunner treats a
                // completed load as PASS regardless of script errors.
                return Ok(Completed {
                    status: Status::Pass,
                    message: None,
                    subtests: vec![Subtest {
                        name: "Crashtest".into(),
                        status: "PASS".into(),
                        message: None,
                    }],
                });
            }
        }
    }
}

/// Turns the reporter payload into a verdict.
fn parse_result(raw: &str) -> Result<Completed, Failure> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| Failure::Error(format!("bad harness result: {error}")))?;
    let harness_status = value
        .get("status")
        .and_then(Value::as_u64)
        .unwrap_or(3)
        .try_into()
        .unwrap_or(3u8);
    let subtests: Vec<Subtest> = value
        .get("tests")
        .and_then(Value::as_array)
        .map(|tests| tests.iter().map(parse_subtest).collect())
        .unwrap_or_default();
    let status = match harness_status {
        0 if subtests.iter().all(|subtest| subtest.status == "PASS") => Status::Pass,
        0 | 1 => Status::Fail,
        2 => Status::Timeout,
        // `PRECONDITION_FAILED`: optional features the test needs are absent,
        // which wptrunner reports as a skip.
        3 => Status::Skip,
        _ => Status::Error,
    };
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            subtests
                .iter()
                .find(|subtest| subtest.status != "PASS")
                .and_then(|subtest| subtest.message.clone())
        });
    Ok(Completed {
        status,
        message,
        subtests,
    })
}

fn parse_subtest(value: &Value) -> Subtest {
    Subtest {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Test")
            .to_owned(),
        status: match value.get("status").and_then(Value::as_u64).unwrap_or(3) {
            0 => "PASS",
            1 => "FAIL",
            2 => "TIMEOUT",
            4 => "SKIP",
            _ => "ERROR",
        }
        .into(),
        message: value
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.is_empty())
            .map(str::to_owned),
    }
}
