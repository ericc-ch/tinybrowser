//! Selecting WPT files and deciding which executor runs them.
//!
//! The checkout is read-only. Reftests are selected so the summary can count
//! them, but the runner skips them until the screenshot comparison lands.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Which executor a file needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// testharness.js reports subtests through the injected reporter.
    Testharness,
    /// The file only has to load and not kill the process.
    Crashtest,
    /// The file compares against a reference screenshot.
    Reftest,
    /// The file calls `WebDriver` through `testdriver.js`, which this runner does
    /// not implement.
    Testdriver,
}

/// One selected file.
#[derive(Clone, Debug)]
pub struct TestCase {
    /// Path relative to the WPT root, with `/` separators.
    pub relative: String,
    /// The executor to use.
    pub kind: Kind,
    /// Source text, read once before workers start.
    pub source: String,
    /// Whether the file declares `<meta name=timeout content=long>`.
    pub long_timeout: bool,
}

impl TestCase {
    /// The wptreport identifier.
    #[must_use]
    pub fn id(&self) -> String {
        format!("/{}", self.relative)
    }
}

/// Reads every selected file below `root`.
///
/// `paths` are relative to `root` and may name files or directories. An empty
/// list is rejected by the caller so a run cannot accidentally start the full
/// suite.
///
/// # Errors
///
/// A selected path, its directory, or a file cannot be read.
pub fn select(root: &Path, paths: &[String], limit: usize) -> io::Result<Vec<TestCase>> {
    let mut files = Vec::new();
    for path in paths {
        let target = root.join(path);
        if target.is_dir() {
            collect(root, &target, &mut files)?;
        } else if target.is_file() {
            push(root, &target, &mut files)?;
        } else {
            return Err(io::Error::other(format!(
                "{} is not a file or directory",
                target.display()
            )));
        }
    }
    files.sort();
    files.dedup();
    files.truncate(limit);
    let mut cases = Vec::with_capacity(files.len());
    for relative in files {
        let source = fs::read_to_string(root.join(&relative))?;
        let kind = classify(&source);
        let long_timeout = declares_long_timeout(&source);
        cases.push(TestCase {
            relative,
            kind,
            source,
            long_timeout,
        });
    }
    Ok(cases)
}

fn collect(root: &Path, directory: &Path, files: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files)?;
        } else {
            push(root, &path, files)?;
        }
    }
    Ok(())
}

fn push(root: &Path, path: &Path, files: &mut Vec<String>) -> io::Result<()> {
    let extension = path.extension().and_then(|extension| extension.to_str());
    if !matches!(extension, Some("html" | "xhtml")) {
        return Ok(());
    }
    let relative = path
        .strip_prefix(root)
        .map_err(io::Error::other)?
        .to_string_lossy()
        .replace('\\', "/");
    files.push(relative);
    Ok(())
}

/// Decides which executor a file needs from its own markup.
fn classify(source: &str) -> Kind {
    let lower = source.to_ascii_lowercase();
    let reftest = ["rel=\"match\"", "rel='match'", "rel=match", "rel=\"mismatch\"", "rel='mismatch'", "rel=mismatch"]
        .iter()
        .any(|marker| lower.contains(marker));
    if reftest {
        return Kind::Reftest;
    }
    if lower.contains("testdriver.js")
        || lower.contains("testdriver-actions.js")
        || lower.contains("testdriver-vendor.js")
    {
        return Kind::Testdriver;
    }
    if lower.contains("testharness.js") || lower.contains("testharnessreport.js") {
        return Kind::Testharness;
    }
    Kind::Crashtest
}

/// Whether a file asks for the long harness budget.
fn declares_long_timeout(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.contains("timeout")
        && (lower.contains("content=long") || lower.contains("content=\"long\"") || lower.contains("content='long'"))
}

/// The default checkout location, relative to the repository root.
#[must_use]
pub fn default_root() -> PathBuf {
    PathBuf::from("third_party/wpt")
}

#[cfg(test)]
mod tests {
    use super::{Kind, classify, declares_long_timeout};

    #[test]
    fn testharness_crashtest_and_reftest_are_distinguished() {
        assert_eq!(
            classify("<script src=/resources/testharness.js></script>"),
            Kind::Testharness
        );
        assert_eq!(
            classify("<link rel=match href=reference.html>"),
            Kind::Reftest
        );
        assert_eq!(
            classify("<link rel='mismatch' href=reference.html>"),
            Kind::Reftest
        );
        assert_eq!(classify("<p>just load</p>"), Kind::Crashtest);
        assert_eq!(
            classify("<script src=/resources/testdriver.js></script>"),
            Kind::Testdriver
        );
    }

    #[test]
    fn long_timeout_is_read_from_the_meta_tag() {
        assert!(declares_long_timeout("<meta name=timeout content=long>"));
        assert!(declares_long_timeout(
            "<meta content=\"long\" name=\"timeout\">"
        ));
        assert!(!declares_long_timeout("<meta name=timeout content=default>"));
        assert!(!declares_long_timeout("<p>long</p>"));
    }
}

