//! wptreport output, the baseline diff, and the console summary.
//!
//! Both fast lanes (`wpt-fast`, `wpt-harness`) write the same report shape and
//! gate on the same baseline rule, so the implementation lives here once. Two
//! copies had already drifted: one learned that a file-level timeout, crash, or
//! error outranks its subtests, and the other still read such a baseline as a
//! pass, so a later failure was reported as a regression and a later pass was
//! never reported as a fix (#234).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The file-level verdict for one test.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Status {
    /// The test completed and every subtest passed.
    Pass,
    /// The test completed with at least one failure.
    Fail,
    /// The runner killed the test at its budget.
    Timeout,
    /// The engine or the runner could not execute the test.
    Error,
    /// The runner does not implement this test's kind.
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

    /// Whether the file stopped before it could report a verdict, which
    /// outranks whatever subtests it managed to emit.
    #[must_use]
    pub fn is_hard(self) -> bool {
        matches!(self, Self::Timeout | Self::Error)
    }
}

/// Which executor a test needs. The cache stores this, so an unknown spelling
/// fails to deserialize into a cache miss instead of silently becoming a
/// testharness file.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// testharness.js reports subtests through the injected reporter.
    Testharness,
    /// The file only has to load and not kill the process.
    Crashtest,
    /// The file compares against a reference screenshot.
    Reftest,
    /// The file calls `WebDriver` through `testdriver.js`.
    Testdriver,
}

/// The verdict one subtest reported.
///
/// testharness.js encodes these as numbers; the report spells them out. The
/// two conversions live here so the cache and the report writer cannot
/// disagree, and an unknown code is a harness error rather than a default.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SubtestStatus {
    /// The subtest passed.
    Pass,
    /// The subtest failed.
    Fail,
    /// The subtest exceeded its timeout.
    Timeout,
    /// The harness never reached the subtest.
    NotRun,
    /// A required feature was absent, which wptrunner reports as a skip.
    PreconditionFailed,
    /// A test262 variant that the runner excluded before running.
    Skip,
}

impl SubtestStatus {
    /// The spelling a wptreport uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Timeout => "TIMEOUT",
            Self::NotRun => "NOTRUN",
            // wptrunner reports both as skips: a missing optional feature and a
            // variant the runner excluded before it ran.
            Self::PreconditionFailed | Self::Skip => "SKIP",
        }
    }

    /// Reads the numeric code testharness.js reports, or `None` for a code the
    /// harness does not define.
    #[must_use]
    pub fn from_harness_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(Self::Pass),
            1 => Some(Self::Fail),
            2 => Some(Self::Timeout),
            3 => Some(Self::NotRun),
            4 => Some(Self::PreconditionFailed),
            _ => None,
        }
    }
}

/// One testharness subtest.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Subtest {
    /// Subtest name.
    pub name: String,
    /// The subtest's verdict.
    pub status: SubtestStatus,
    /// Failure message.
    pub message: Option<String>,
}

/// One file's result.
///
/// `Deserialize` as well as `Serialize`: the harness journals finished files so
/// a process that aborts can resume, and a resumed verdict has to survive the
/// round trip unchanged.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Outcome {
    /// wptreport test identifier, such as `/dom/nodes/Node-nodeName.html`.
    pub id: String,
    /// File-level verdict used by the summary and the baseline diff.
    pub status: Status,
    /// Failure, skip, or timeout reason.
    pub message: Option<String>,
    /// Wall time spent in this file.
    pub duration: Duration,
    /// testharness subtests, or one synthetic subtest for a non-testharness
    /// lane that reports through the same shape.
    pub subtests: Vec<Subtest>,
    /// Whether the overall `status` field is `OK` and the verdict lives in the
    /// subtests, which is the testharness form. A crashtest uses the
    /// file-level form WPT already emits.
    pub testharness: bool,
}

/// Writes `outcomes` as a wptreport the `tools/wpt/cli.py` readers understand.
///
/// # Errors
///
/// The report cannot be created or written.
pub fn write_report(path: &Path, outcomes: &[Outcome]) -> io::Result<()> {
    let results: Vec<Value> = outcomes
        .iter()
        .map(|outcome| {
            let overall = if outcome.testharness {
                match outcome.status {
                    Status::Pass | Status::Fail => "OK",
                    Status::Timeout => "TIMEOUT",
                    Status::Error => "ERROR",
                    Status::Skip => "SKIP",
                }
            } else {
                outcome.status.as_str()
            };
            let subtests: Vec<Value> = if outcome.testharness {
                outcome
                    .subtests
                    .iter()
                    .map(|subtest| {
                        json!({
                            "name": subtest.name,
                            "status": subtest.status.as_str(),
                            "message": subtest.message,
                            "expected": "PASS",
                            "known_intermittent": [],
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            json!({
                "test": outcome.id,
                "subsuite": "",
                "status": overall,
                "message": outcome.message,
                "duration": outcome.duration.as_secs_f64() * 1000.0,
                "known_intermittent": [],
                "subtests": subtests,
            })
        })
        .collect();
    let report = json!({ "results": results });
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, &report)?;
    writer.write_all(b"\n")
}

/// What changed against a baseline report.
pub struct Comparison {
    /// Tests that were passing (or skipped) and now fail or stop running.
    pub regressions: Vec<String>,
    /// Tests that were failing and now pass.
    pub fixes: Vec<String>,
    /// Tests that are not in the baseline and fail now.
    pub new_failures: Vec<String>,
    /// Baseline tests the run did not select.
    pub missing: Vec<String>,
}

impl Comparison {
    /// Whether a push should be rejected.
    #[must_use]
    pub fn failed(&self) -> bool {
        !self.regressions.is_empty() || !self.new_failures.is_empty()
    }
}

/// Compares `outcomes` with a previous wptreport.
///
/// # Errors
///
/// The baseline cannot be read or does not have the expected shape.
pub fn compare(baseline: &Path, outcomes: &[Outcome]) -> io::Result<Comparison> {
    let text = std::fs::read_to_string(baseline)?;
    let report: Value = serde_json::from_str(&text).map_err(io::Error::other)?;
    let results = report
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("baseline has no results array"))?;
    let mut previous = BTreeMap::new();
    for result in results {
        let Some(id) = result.get("test").and_then(Value::as_str) else {
            continue;
        };
        previous.insert(id.to_owned(), verdict(result).to_owned());
    }
    let mut comparison = Comparison {
        regressions: Vec::new(),
        fixes: Vec::new(),
        new_failures: Vec::new(),
        missing: Vec::new(),
    };
    let mut seen = BTreeSet::new();
    for outcome in outcomes {
        seen.insert(outcome.id.as_str());
        let current = outcome.status.as_str();
        match previous.get(&outcome.id).map(String::as_str) {
            Some("PASS" | "SKIP") if current == "FAIL" => {
                comparison.regressions.push(outcome.id.clone());
            }
            // A test that used to run and pass but is now skipped or stopped
            // short is lost coverage, which is a regression for the per-push
            // gate.
            Some("PASS") if matches!(current, "SKIP" | "TIMEOUT" | "ERROR") => {
                comparison.regressions.push(outcome.id.clone());
            }
            // A hard baseline that now passes is a fix too; a TIMEOUT or an
            // ERROR is a failing baseline, not a different category.
            Some("FAIL" | "TIMEOUT" | "ERROR" | "CRASH") if current == "PASS" => {
                comparison.fixes.push(outcome.id.clone());
            }
            None if current == "FAIL" => comparison.new_failures.push(outcome.id.clone()),
            _ => {}
        }
    }
    for test in previous.keys() {
        if !seen.contains(test.as_str()) {
            comparison.missing.push(test.clone());
        }
    }
    Ok(comparison)
}

/// The file-level verdict one report entry records.
///
/// A testharness entry keeps the overall `OK` status and reports the verdict in
/// its subtests, so an entry fails when any subtest is neither `PASS` nor
/// `SKIP`. Reading one fixed subtest instead would call a file whose later
/// subtest fails a pass, and every later run would report the same false
/// regression. `tools/wpt/cli.py`'s `failing_tests` reads the same rule, which
/// is what makes a baseline written by one reader usable by the other.
fn verdict(result: &Value) -> &str {
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("SKIP");
    // A file-level timeout, crash, or error outranks its subtests: the file
    // stopped part-way, so the subtests it reported are a prefix, not a
    // verdict.
    if matches!(status, "TIMEOUT" | "ERROR" | "CRASH") {
        return status;
    }
    let mut passed = matches!(status, "OK" | "PASS");
    if let Some(subtests) = result.get("subtests").and_then(Value::as_array) {
        for subtest in subtests {
            match subtest.get("status").and_then(Value::as_str) {
                Some("PASS") => passed = true,
                Some("SKIP") | None => {}
                Some(_) => return "FAIL",
            }
        }
    }
    if passed { "PASS" } else { "SKIP" }
}

/// Prints the run summary and, when `verbose`, the failures and slowest tests.
pub fn summarize(outcomes: &[Outcome], wall: Duration, verbose: bool) {
    let mut pass = 0;
    let mut fail = 0;
    let mut timeout = 0;
    let mut error = 0;
    let mut skip = 0;
    let mut failures = Vec::new();
    let mut slowest: Vec<&Outcome> = outcomes.iter().collect();
    for outcome in outcomes {
        match outcome.status {
            Status::Pass => pass += 1,
            Status::Fail => {
                fail += 1;
                failures.push(outcome);
            }
            Status::Timeout => timeout += 1,
            Status::Error => error += 1,
            Status::Skip => skip += 1,
        }
    }
    slowest.sort_by_key(|outcome| std::cmp::Reverse(outcome.duration));
    let count = u32::try_from(outcomes.len()).unwrap_or(u32::MAX);
    println!(
        "{} tests in {:.1?} ({:.2} ms/test): {pass} pass, {fail} fail, {timeout} timeout, \
         {error} error, {skip} skip",
        outcomes.len(),
        wall,
        wall.as_secs_f64() * 1000.0 / f64::from(count)
    );
    if verbose {
        for outcome in failures {
            println!(
                "FAIL {} :: {}",
                outcome.id,
                outcome.message.as_deref().unwrap_or("")
            );
        }
        for outcome in slowest.iter().take(10) {
            println!("SLOW {:>8.1?} {}", outcome.duration, outcome.id);
        }
    }
}

/// Prints what changed against the baseline.
pub fn print_comparison(comparison: &Comparison) {
    for test in &comparison.regressions {
        println!("REGRESSION {test}");
    }
    for test in &comparison.new_failures {
        println!("NEW-FAILURE {test}");
    }
    for test in &comparison.fixes {
        println!("FIXED {test}");
    }
    println!(
        "baseline diff: {} regressions, {} new failures, {} fixes, {} tests not selected",
        comparison.regressions.len(),
        comparison.new_failures.len(),
        comparison.fixes.len(),
        comparison.missing.len()
    );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Outcome, Status, compare};

    fn outcome(id: &str, status: Status) -> Outcome {
        Outcome {
            id: id.into(),
            status,
            message: None,
            duration: Duration::ZERO,
            subtests: Vec::new(),
            testharness: true,
        }
    }

    fn scratch(name: &str, contents: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("wpt-report-{name}-{}.json", std::process::id()));
        std::fs::write(&path, contents).expect("baseline written");
        path
    }

    #[test]
    fn a_diff_separates_regressions_from_fixes() {
        let path = scratch(
            "compare",
            r#"{"results":[
                {"test":"a","subtests":[{"status":"PASS"}]},
                {"test":"b","subtests":[{"status":"FAIL"}]},
                {"test":"c","subtests":[{"status":"SKIP"}]},
                {"test":"d","subtests":[{"status":"PASS"}]},
                {"test":"mixed","subtests":[{"status":"PASS"},{"status":"FAIL"}]},
                {"test":"timed","status":"TIMEOUT","subtests":[]},
                {"test":"broken","status":"ERROR","subtests":[{"status":"PASS"}]},
                {"test":"gone","subtests":[{"status":"PASS"}]}
            ]}"#,
        );
        let outcomes = [
            outcome("a", Status::Fail),
            outcome("b", Status::Pass),
            outcome("c", Status::Pass),
            outcome("d", Status::Skip),
            outcome("mixed", Status::Fail),
            outcome("timed", Status::Pass),
            outcome("broken", Status::Fail),
            outcome("new", Status::Fail),
        ];
        let comparison = compare(&path, &outcomes).expect("compared");
        std::fs::remove_file(&path).expect("baseline removed");
        assert_eq!(comparison.regressions, ["a", "d"]);
        assert_eq!(comparison.fixes, ["b", "timed"]);
        assert_eq!(comparison.new_failures, ["new"]);
        assert_eq!(comparison.missing, ["gone"]);
        assert!(comparison.failed());
    }

    /// A hard status in the baseline outranks the subtests it emitted before
    /// dying, so a later pass is a fix and a later failure is not a
    /// regression.
    #[test]
    fn a_hard_status_outranks_its_subtests() {
        let path = scratch(
            "hard",
            r#"{"results":[
                {"test":"broken","status":"ERROR","subtests":[{"status":"PASS"}]},
                {"test":"timed","status":"TIMEOUT","subtests":[]},
                {"test":"crashed","status":"CRASH","subtests":[]}
            ]}"#,
        );
        let outcomes = [
            outcome("broken", Status::Pass),
            outcome("timed", Status::Fail),
            outcome("crashed", Status::Skip),
        ];
        let comparison = compare(&path, &outcomes).expect("compared");
        std::fs::remove_file(&path).expect("baseline removed");
        assert_eq!(comparison.fixes, ["broken"]);
        assert_eq!(comparison.regressions, [] as [String; 0]);
        assert_eq!(comparison.new_failures, [] as [String; 0]);
    }
}
