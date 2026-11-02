//! Test selection from a recorded test-to-source map.
//!
//! The map says which repository files each test executed. A push selects the
//! tests that executed any changed file.
//!
//! The rule is conservative in the one place that matters: a changed path that
//! no test lists is unknown (a new module, or one never covered), so the
//! selection falls back to the whole suite instead of silently running less
//! than it should.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use serde::Deserialize;

const VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct File {
    version: u32,
    tests: BTreeMap<String, Vec<String>>,
}

/// The recorded map, inverted so a changed path finds its tests.
pub struct Deps {
    /// Repository path -> tests that executed it.
    by_file: BTreeMap<String, BTreeSet<String>>,
    /// Every file the map knows about.
    known: BTreeSet<String>,
}

impl Deps {
    /// Reads a map written by `tools/wpt/deps.py`.
    ///
    /// # Errors
    ///
    /// The file cannot be read, is not valid JSON, or has a different format
    /// version.
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let file: File = serde_json::from_str(&text).map_err(io::Error::other)?;
        if file.version != VERSION {
            return Err(io::Error::other(format!(
                "dependency map version {} is not {VERSION}",
                file.version
            )));
        }
        let mut by_file: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (test, files) in file.tests {
            for source in files {
                by_file.entry(source).or_default().insert(test.clone());
            }
        }
        let known = by_file.keys().cloned().collect();
        Ok(Self { by_file, known })
    }

    /// The tests to run for `changed`, or `None` when the whole suite must run.
    ///
    /// `None` means at least one changed file has no recorded test, so the map
    /// cannot bound the blast radius.
    #[must_use]
    pub fn select(&self, changed: &[String]) -> Option<BTreeSet<String>> {
        let mut selected = BTreeSet::new();
        for path in changed {
            selected.extend(self.by_file.get(path)?.iter().cloned());
        }
        Some(selected)
    }

    /// The files the map knows about, for reporting how complete it is.
    #[must_use]
    pub fn known(&self) -> &BTreeSet<String> {
        &self.known
    }
}
