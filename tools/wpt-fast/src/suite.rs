//! Selecting test262 files from a checkout.
//!
//! The suite is read-only. Nothing here writes under the checkout: strict
//! variants are a prefix added while evaluating, not generated files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::meta;
use crate::runner::TestCase;

/// Reads every selected file below `root`.
///
/// `filters` match a path relative to the test262 root (`test/...` or
/// `harness/...`) after a leading slash is removed. An empty list selects all.
///
/// # Errors
///
/// The suite root, one of its directories, or one selected file cannot be
/// read.
pub fn select(root: &Path, filters: &[String]) -> io::Result<Vec<TestCase>> {
    let mut files = Vec::new();
    for directory in ["test", "harness"] {
        let path = root.join(directory);
        if path.is_dir() {
            collect(&path, &mut files)?;
        }
    }
    files.sort();
    let mut cases = Vec::new();
    for relative in files {
        if !filters.is_empty() && !filters.iter().any(|filter| relative.starts_with(filter)) {
            continue;
        }
        let source = fs::read_to_string(root.join(&relative))?;
        let meta = meta::parse(&source);
        // WPT runs one variant per file: strict only for `onlyStrict`, sloppy
        // for everything else (noStrict is already the default).
        let strict = meta.is_only_strict();
        cases.push(TestCase {
            relative,
            strict,
            source,
            meta,
        });
    }
    Ok(cases)
}

/// Appends every non-fixture `.js` below `directory`, relative to its parent.
fn collect(directory: &Path, files: &mut Vec<String>) -> io::Result<()> {
    let base = directory
        .parent()
        .ok_or_else(|| io::Error::other("test262 root has no parent"))?;
    collect_below(base, directory, files)
}

fn collect_below(base: &Path, directory: &Path, files: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_below(base, &path, files)?;
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "js") {
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("_FIXTURE.js"))
        {
            continue;
        }
        let relative = path
            .strip_prefix(base)
            .map_err(io::Error::other)?
            .to_string_lossy()
            .replace('\\', "/");
        files.push(relative);
    }
    Ok(())
}

/// The default checkout location, relative to the repository root.
#[must_use]
pub fn default_root() -> PathBuf {
    PathBuf::from("third_party/wpt/third_party/test262")
}
