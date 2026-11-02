//! wptreport output and the per-push baseline diff.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use serde_json::{Value, json};

use crate::runner::{Outcome, Status};

/// Writes `outcomes` as a wptreport the existing `tools/wpt/cli.py` readers
/// already understand.
///
/// # Errors
///
/// The report cannot be created or written.
pub fn write_report(path: &Path, outcomes: &[Outcome]) -> io::Result<()> {
    let results: Vec<Value> = outcomes
        .iter()
        .map(|outcome| {
            let status = if outcome.status == Status::Skip {
                "SKIP"
            } else {
                "OK"
            };
            json!({
                "test": outcome.id,
                "subsuite": "",
                "status": status,
                "message": Value::Null,
                "duration": outcome.duration.as_secs_f64() * 1000.0,
                "known_intermittent": [],
                "subtests": [{
                    "name": "Test",
                    "status": outcome.status.as_str(),
                    "message": outcome.message,
                    "expected": "PASS",
                    "known_intermittent": [],
                }],
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
    /// Tests that were passing (or skipped) and now fail.
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

/// Compares `outcomes` with a previous report written by [`write_report`].
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
        let status = result
            .get("subtests")
            .and_then(Value::as_array)
            .and_then(|subtests| subtests.first())
            .and_then(|subtest| subtest.get("status"))
            .and_then(Value::as_str)
            .or_else(|| result.get("status").and_then(Value::as_str))
            .unwrap_or("SKIP");
        previous.insert(id.to_owned(), status.to_owned());
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
            // A test that used to run and pass but is now skipped is lost
            // coverage, which is a regression for the per-push gate.
            Some("PASS") if current == "SKIP" => {
                comparison.regressions.push(outcome.id.clone());
            }
            Some("FAIL") if current == "PASS" => comparison.fixes.push(outcome.id.clone()),
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{compare, Status};

    use crate::runner::Outcome;

    fn outcome(id: &str, status: Status) -> Outcome {
        Outcome {
            id: id.into(),
            status,
            message: None,
            duration: Duration::ZERO,
        }
    }

    #[test]
    fn a_diff_separates_regressions_from_fixes() {
        let path = std::env::temp_dir().join(format!(
            "wpt-fast-compare-test-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"results":[
                {"test":"a","subtests":[{"status":"PASS"}]},
                {"test":"b","subtests":[{"status":"FAIL"}]},
                {"test":"c","subtests":[{"status":"SKIP"}]},
                {"test":"d","subtests":[{"status":"PASS"}]},
                {"test":"gone","subtests":[{"status":"PASS"}]}
            ]}"#,
        )
        .expect("baseline written");
        let outcomes = [
            outcome("a", Status::Fail),
            outcome("b", Status::Pass),
            outcome("c", Status::Pass),
            outcome("d", Status::Skip),
            outcome("new", Status::Fail),
        ];
        let comparison = compare(&path, &outcomes).expect("compared");
        std::fs::remove_file(&path).expect("baseline removed");
        assert_eq!(comparison.regressions, ["a", "d"]);
        assert_eq!(comparison.fixes, ["b"]);
        assert_eq!(comparison.new_failures, ["new"]);
        assert_eq!(comparison.missing, ["gone"]);
        assert!(comparison.failed());
    }
}
