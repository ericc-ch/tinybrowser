//! In-process WPT testharness runner.
//!
//! Mounts each selected file in the real `renderer::Engine` without `WebDriver`,
//! wptserve, or a renderer child process. A paused Tokio clock jumps over
//! timer waits, so a hung test costs milliseconds instead of the harness
//! budget.
//!
//! The checkout is read-only. Requests are served from memory and the only
//! replacement is `/resources/testharnessreport.js`, which records the
//! completion payload the host polls.

mod cache;
mod engine;
mod services;
mod suite;
mod template;
mod wrapper;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cache::Cache;
use engine::Outcome;
use suite::{Kind, TestCase};

/// Parsed command line.
struct Options {
    root: PathBuf,
    paths: Vec<String>,
    jobs: usize,
    report: Option<PathBuf>,
    baseline: Option<PathBuf>,
    cache: Option<PathBuf>,
    verbose: bool,
    list: bool,
    limit: usize,
    offset: usize,
    mark_crashed: Option<String>,
    virtual_time: bool,
    budgets: engine::Budgets,
    content_types: HashMap<String, String>,
}

fn main() {
    let options = match parse_args() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("wpt-harness: {message}");
            eprintln!("{HELP}");
            std::process::exit(2);
        }
    };
    if !options.root.is_dir() {
        eprintln!(
            "wpt-harness: {} is not a WPT checkout; pass --wpt-root",
            options.root.display()
        );
        std::process::exit(2);
    }
    if let Some(id) = &options.mark_crashed {
        let Some(path) = &options.cache else {
            eprintln!("wpt-harness: --mark-crashed needs --cache");
            exit(2);
        };
        let binary = match cache::binary_digest(Some(path)) {
            Ok(binary) => binary,
            Err(error) => {
                eprintln!("wpt-harness: cannot hash the harness binary: {error}");
                exit(2);
            }
        };
        let mut stored = match Cache::load(path, &binary) {
            Ok(cache) => cache,
            Err(error) => {
                eprintln!("wpt-harness: cannot read {}: {error}", path.display());
                exit(2);
            }
        };
        stored.mark_crashed(id, &options.root);
        if let Err(error) = stored.save(path) {
            eprintln!("wpt-harness: cannot write {}: {error}", path.display());
            exit(2);
        }
        exit(0);
    }
    let cases = match suite::select(
        &options.root,
        &options.paths,
        options.offset,
        options.limit,
        &options.content_types,
    ) {
        Ok(cases) => cases,
        Err(error) => {
            eprintln!("wpt-harness: cannot read suite: {error}");
            std::process::exit(2);
        }
    };
    if cases.is_empty() {
        eprintln!("wpt-harness: no tests selected");
        std::process::exit(2);
    }
    if options.list {
        for case in &cases {
            println!("{:?}\t{}", case.kind, case.relative);
        }
        return;
    }
    let started = Instant::now();
    let (outcomes, cached) = run_selected(&options, &cases);
    let wall = started.elapsed();
    let reported: Vec<_> = outcomes.iter().map(reported).collect();
    wpt_report::summarize(&reported, wall, cached, options.verbose);
    if let Some(path) = &options.report
        && let Err(error) = wpt_report::write_report(path, &reported)
    {
        eprintln!("wpt-harness: cannot write {}: {error}", path.display());
        std::process::exit(2);
    }
    if let Some(baseline) = &options.baseline {
        match wpt_report::compare(baseline, &reported) {
            Ok(comparison) => {
                wpt_report::print_comparison(&comparison);
                if comparison.failed() {
                    exit(1);
                }
            }
            Err(error) => {
                eprintln!(
                    "wpt-harness: cannot compare {}: {error}",
                    baseline.display()
                );
                std::process::exit(2);
            }
        }
    }
    exit(0);
}

/// Leaves without running destructors.
///
/// A run's last engine can still root a `QuickJS` object, and freeing the
/// runtime then trips `QuickJS`'s own `gc_obj_list` assertion — after every
/// report is written. A browser process exits the same way, without walking
/// its heap, so the harness does not turn a finished run into a crash.
fn exit(code: i32) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    std::process::exit(code)
}

/// Runs every case, reusing cached verdicts whose binary and inputs match.
///
/// Returns the outcomes in test order and how many came from the cache.
fn run_selected(options: &Options, cases: &[TestCase]) -> (Vec<Outcome>, usize) {
    let binary = match cache::binary_digest(options.cache.as_deref()) {
        Ok(binary) => binary,
        Err(error) => {
            eprintln!("wpt-harness: cannot hash the harness binary: {error}");
            std::process::exit(2);
        }
    };
    let mut stored = match &options.cache {
        Some(path) => match Cache::load(path, &binary) {
            Ok(cache) => Some(cache),
            Err(error) => {
                eprintln!("wpt-harness: cannot read {}: {error}", path.display());
                std::process::exit(2);
            }
        },
        None => None,
    };
    // A cached verdict is reused only when the binary and every file the test
    // read are unchanged, so a warm run costs hashing instead of execution.
    let mut slots: Vec<Option<Outcome>> = (0..cases.len()).map(|_| None).collect();
    let mut pending = Vec::new();
    if let Some(cache) = &stored {
        for (index, case) in cases.iter().enumerate() {
            match cache.lookup(&case.id(), &options.root) {
                Some(outcome) => slots[index] = Some(outcome),
                None => pending.push(index),
            }
        }
    } else {
        pending.extend(0..cases.len());
    }
    let cached = cases.len() - pending.len();
    if cached > 0 {
        eprintln!("wpt-harness: {cached} cached, {} to run", pending.len());
    }
    let selected: Vec<TestCase> = pending.iter().map(|index| cases[*index].clone()).collect();
    let fresh = run_all(
        &options.root,
        &selected,
        options.jobs,
        options.virtual_time,
        options.budgets,
        &options.content_types,
    );
    for (index, outcome) in pending.into_iter().zip(fresh) {
        slots[index] = Some(outcome);
    }
    let outcomes: Vec<Outcome> = slots
        .into_iter()
        .enumerate()
        .map(|(index, outcome)| {
            outcome.unwrap_or_else(|| Outcome {
                id: cases[index].id(),
                kind: cases[index].kind,
                status: engine::Status::Skip,
                message: Some("worker stopped before this test".into()),
                subtests: Vec::new(),
                duration: std::time::Duration::ZERO,
                inputs: Vec::new(),
            })
        })
        .collect();
    if let Some(cache) = &mut stored {
        for outcome in &outcomes {
            cache.insert(outcome, &options.root);
        }
        if let Some(path) = &options.cache
            && let Err(error) = cache.save(path)
        {
            eprintln!("wpt-harness: cannot write {}: {error}", path.display());
            std::process::exit(2);
        }
    }
    (outcomes, cached)
}

/// Runs every case, one current-thread runtime per worker.
fn run_all(
    root: &std::path::Path,
    cases: &[TestCase],
    jobs: usize,
    virtual_time: bool,
    budgets: engine::Budgets,
    content_types: &HashMap<String, String>,
) -> Vec<Outcome> {
    if cases.is_empty() {
        return Vec::new();
    }
    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel();
    let workers = jobs.clamp(1, cases.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let sender = sender.clone();
            let next = &next;
            scope.spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .start_paused(virtual_time)
                    .build()
                    .unwrap_or_else(|error| {
                        eprintln!("wpt-harness: cannot create runtime: {error}");
                        std::process::exit(2);
                    });
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(case) = cases.get(index) else {
                        break;
                    };
                    let started = Instant::now();
                    // One engine per file. Reusing one engine across a worker's
                    // files is faster, but a page can leave a `QuickJS` class
                    // cell borrowed, and the next mount then panics inside
                    // rquickjs. A file's own engine cannot see that state.
                    let mut session = engine::Session::new(root, content_types.clone(), budgets);
                    // Layout, paint, and the bindings can panic on a hostile
                    // page. A panic is this file's ERROR, not the end of the
                    // run; the engine that saw it is abandoned rather than
                    // dropped, because dropping a half-torn heap can panic
                    // again during unwinding.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        runtime.block_on(session.run(case, virtual_time))
                    }));
                    let outcome = result.unwrap_or_else(|_| {
                        Box::leak(Box::new(session));
                        Outcome {
                            id: case.id(),
                            kind: case.kind,
                            status: engine::Status::Error,
                            message: Some("engine panicked".into()),
                            subtests: Vec::new(),
                            duration: started.elapsed(),
                            inputs: Vec::new(),
                        }
                    });
                    if sender.send((index, outcome)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut slots: Vec<Option<Outcome>> = (0..cases.len()).map(|_| None).collect();
        for (index, outcome) in receiver {
            slots[index] = Some(outcome);
        }
        slots
            .into_iter()
            .enumerate()
            .map(|(index, outcome)| {
                outcome.unwrap_or_else(|| Outcome {
                    id: cases[index].id(),
                    kind: cases[index].kind,
                    status: engine::Status::Skip,
                    message: Some("worker stopped before this test".into()),
                    subtests: Vec::new(),
                    duration: std::time::Duration::ZERO,
                    inputs: Vec::new(),
                })
            })
            .collect()
    })
}

/// Prints counts and the slowest files.
/// The shared report shape for one engine outcome.
///
/// The engine keeps its own `kind` (it drives the executor) and `inputs` (the
/// result cache), which a wptreport has no field for.
fn reported(outcome: &Outcome) -> wpt_report::Outcome {
    wpt_report::Outcome {
        id: outcome.id.clone(),
        status: outcome.status,
        message: outcome.message.clone(),
        duration: outcome.duration,
        subtests: outcome
            .subtests
            .iter()
            .map(|subtest| wpt_report::Subtest {
                name: subtest.name.clone(),
                status: subtest.status,
                message: subtest.message.clone(),
            })
            .collect(),
        testharness: matches!(outcome.kind, Kind::Testharness),
    }
}

const HELP: &str = "\
usage: wpt-harness [OPTIONS] PATH...

Runs WPT testharness and crashtest files in-process.

  --wpt-root PATH   WPT checkout (default third_party/wpt)
  --jobs N          worker threads (default: available CPUs)
  --report PATH     write a wptreport JSON
  --baseline PATH   compare with an earlier --report and exit 1 on regressions
  --cache PATH      reuse unchanged verdicts from this cache file (and update it)
  --limit N         stop after N selected files
  --offset N        skip the first N selected files (shards)
  --mark-crashed ID store an ERROR verdict for ID in --cache and exit
  --normal-budget S wall-clock seconds before a normal file is TIMEOUT
  --long-budget S  wall-clock seconds before a long-timeout file is TIMEOUT
  --content-types JSON  wptserve's extension-to-content-type map
  --real-time       use the wall clock instead of the paused clock
  --verbose         print failing tests and the ten slowest files
  --list            print the selected files and exit
  -h, --help        show this help

PATH is relative to the WPT root, such as dom/nodes or dom/events/foo.html.
Tests come from MANIFEST.json, which `cli.py manifest` builds and stamps;
reftests and generated worker wrappers are not selected.";

/// Parses the command line.
/// Takes the next argument as a path.
fn path_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} needs a path"))
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options {
        root: suite::default_root(),
        paths: Vec::new(),
        jobs: std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        report: None,
        baseline: None,
        cache: None,
        verbose: false,
        list: false,
        limit: usize::MAX,
        offset: 0,
        mark_crashed: None,
        virtual_time: true,
        // Replaced by the required flags below. Zero is never a real budget:
        // `for_case` would time out before mounting, and the flag parser
        // rejects it.
        budgets: engine::Budgets {
            normal: Duration::ZERO,
            long: Duration::ZERO,
        },
        content_types: HashMap::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--wpt-root" => options.root = path_arg(&mut args, "--wpt-root")?,
            "--jobs" => {
                options.jobs = args
                    .next()
                    .ok_or("--jobs needs a number")?
                    .parse()
                    .map_err(|_| "--jobs needs a number")?;
            }
            "--report" => options.report = Some(path_arg(&mut args, "--report")?),
            "--baseline" => options.baseline = Some(path_arg(&mut args, "--baseline")?),
            "--cache" => options.cache = Some(path_arg(&mut args, "--cache")?),
            "--limit" => {
                options.limit = args
                    .next()
                    .ok_or("--limit needs a number")?
                    .parse()
                    .map_err(|_| "--limit needs a number")?;
            }
            "--offset" => {
                options.offset = args
                    .next()
                    .ok_or("--offset needs a number")?
                    .parse()
                    .map_err(|_| "--offset needs a number")?;
            }
            "--mark-crashed" => {
                options.mark_crashed = Some(args.next().ok_or("--mark-crashed needs a test id")?);
            }
            "--normal-budget" => {
                options.budgets.normal = seconds_arg(&mut args, "--normal-budget")?;
            }
            "--long-budget" => options.budgets.long = seconds_arg(&mut args, "--long-budget")?,
            "--content-types" => {
                let raw = args.next().ok_or("--content-types needs a JSON map")?;
                options.content_types = serde_json::from_str(&raw)
                    .map_err(|error| format!("--content-types is not a JSON map: {error}"))?;
            }
            "--real-time" => options.virtual_time = false,
            "--verbose" => options.verbose = true,
            "--list" => options.list = true,
            other if other.starts_with('-') => return Err(format!("unknown flag {other}")),
            other => options.paths.push(other.trim_start_matches('/').to_owned()),
        }
    }
    if options.paths.is_empty() && options.mark_crashed.is_none() {
        return Err("pass at least one test path".into());
    }
    if options.budgets.normal.is_zero() || options.budgets.long.is_zero() {
        return Err("pass --normal-budget and --long-budget in seconds \
             (cli.py computes them from tools/wpt/timeouts.py)"
            .into());
    }
    if options.content_types.is_empty() {
        return Err("pass --content-types with wptserve's extension map \
             (cli.py reads it from wptserve.constants)"
            .into());
    }
    Ok(options)
}

/// Takes the next argument as a positive number of seconds.
fn seconds_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<Duration, String> {
    let raw = args.next().ok_or_else(|| format!("{flag} needs seconds"))?;
    let seconds: f64 = raw
        .parse()
        .map_err(|_| format!("{flag} needs seconds, got {raw}"))?;
    Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| format!("{flag} needs a positive number of seconds, got {raw}"))
}
