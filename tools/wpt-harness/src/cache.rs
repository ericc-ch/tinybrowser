//! Per-file result cache keyed by the harness binary and every file a test read.
//!
//! A test's outcome depends on the engine that ran it and on the bytes of every
//! file it loaded, so the key is exactly those two things. The binary hash
//! covers the whole engine and harness; the input list comes from the in-memory
//! host, which is the one place every request goes through.
//!
//! A warm run still reads and hashes the inputs. It never trusts a stale entry:
//! if a helper script, a `.headers` file, or the test itself changed, the file
//! runs again.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::engine::{Outcome, Status, Subtest};
use crate::suite::Kind;

/// Bumped whenever the meaning of a stored outcome changes.
const VERSION: u32 = 2;

/// One file a test read and the digest it had.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Input {
    /// Path relative to the WPT root, or absolute when it is outside it.
    path: String,
    /// Hex SHA-256, or `missing` when the file did not exist.
    hash: String,
}

/// A stored verdict. The enums carry their report spellings, so a cache with
/// an unknown status fails to deserialize and is a miss instead of silently
/// becoming a skip.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Entry {
    /// Which executor produced the verdict.
    kind: Kind,
    /// File-level status.
    status: Status,
    /// Failure reason.
    message: Option<String>,
    /// Stored subtests.
    subtests: Vec<Subtest>,
    /// Everything the file read.
    inputs: Vec<Input>,
}

/// The cache file.
#[derive(Debug, Deserialize, Serialize)]
pub struct Cache {
    /// Digest of the harness binary that wrote the file.
    binary: String,
    /// Format version.
    version: u32,
    /// Entries keyed by wptreport test id.
    tests: BTreeMap<String, Entry>,
}

impl Cache {
    /// Reads a cache file. A missing file is an empty cache.
    ///
    /// A cache written by a different binary is discarded: its verdicts say
    /// nothing about this engine.
    ///
    /// # Errors
    ///
    /// The file exists but cannot be read or parsed.
    pub fn load(path: &Path, binary: &str) -> io::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::empty(binary));
            }
            Err(error) => return Err(error),
        };
        let mut cache: Self = serde_json::from_str(&text).map_err(io::Error::other)?;
        if cache.version != VERSION || cache.binary != binary {
            return Ok(Self::empty(binary));
        }
        binary.clone_into(&mut cache.binary);
        Ok(cache)
    }

    /// An empty cache that names the binary it belongs to.
    fn empty(binary: &str) -> Self {
        Self {
            binary: binary.to_owned(),
            version: VERSION,
            tests: BTreeMap::new(),
        }
    }

    /// Returns the stored outcome when every input still has the same digest.
    ///
    /// `id` is the wptreport id; `root` resolves relative input paths.
    #[must_use]
    pub fn lookup(&self, id: &str, root: &Path) -> Option<Outcome> {
        let entry = self.tests.get(id)?;
        for input in &entry.inputs {
            let actual = digest(root, &input.path);
            if actual != input.hash {
                if std::env::var_os("WPT_HARNESS_DEBUG").is_some() {
                    eprintln!(
                        "DEBUG cache miss {id}: {} expected {} got {actual}",
                        input.path, input.hash
                    );
                }
                return None;
            }
        }
        Some(Outcome {
            id: id.to_owned(),
            kind: entry.kind,
            status: entry.status,
            message: entry.message.clone(),
            subtests: entry.subtests.clone(),
            duration: std::time::Duration::ZERO,
            inputs: Vec::new(),
        })
    }

    /// Files a fresh outcome under `id`.
    pub fn insert(&mut self, outcome: &Outcome, root: &Path) {
        let mut inputs: Vec<Input> = outcome
            .inputs
            .iter()
            .map(|path| Input {
                path: relative(root, path),
                hash: digest(root, &relative(root, path)),
            })
            .collect();
        inputs.sort_by(|left, right| left.path.cmp(&right.path));
        inputs.dedup_by(|left, right| left.path == right.path);
        self.tests.insert(
            outcome.id.clone(),
            Entry {
                kind: outcome.kind,
                status: outcome.status,
                message: outcome.message.clone(),
                subtests: outcome.subtests.clone(),
                inputs,
            },
        );
    }

    /// Writes an ERROR entry for a file that crashed the engine.
    ///
    /// The process that would have recorded the verdict is gone, so the
    /// driver marks it here after isolating it. Later runs treat it as a
    /// cached verdict instead of crashing again.
    pub fn mark_crashed(&mut self, id: &str, root: &Path) {
        let path = id.trim_start_matches('/').to_owned();
        let inputs = vec![Input {
            hash: digest(root, &path),
            path,
        }];
        self.tests.insert(
            id.to_owned(),
            Entry {
                kind: Kind::Testharness,
                status: Status::Error,
                message: Some("engine crashed in this file".into()),
                subtests: Vec::new(),
                inputs,
            },
        );
    }

    /// Writes the cache file.
    ///
    /// # Errors
    ///
    /// The parent directory or the file cannot be written.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = serde_json::to_string(self).map_err(io::Error::other)?;
        std::fs::write(path, text)
    }
}

/// Hex SHA-256 of the running harness binary.
///
/// Hashing the `wpt` profile binary costs half a second because debug info
/// makes it hundreds of megabytes, so the digest is remembered in
/// `<cache>.binary` next to the cache and reused while the executable's path,
/// length, and modification time are unchanged. A rebuild moves the mtime, so
/// a stale digest is never reused across builds.
///
/// # Errors
///
/// The executable path cannot be read.
pub fn binary_digest(cache: Option<&Path>) -> io::Result<String> {
    let path = std::env::current_exe()?;
    let metadata = std::fs::metadata(&path)?;
    let stamp = format!(
        "{}:{}:{}",
        path.display(),
        metadata.len(),
        metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos())
    );
    let sidecar = cache.map(|cache| PathBuf::from(format!("{}.binary", cache.display())));
    if let Some(sidecar) = &sidecar
        && let Ok(text) = std::fs::read_to_string(sidecar)
        && let Some((saved, hash)) = text.split_once('\n')
        && saved == stamp
    {
        return Ok(hash.to_owned());
    }
    let bytes = std::fs::read(&path)?;
    let hash = format!("{:x}", Sha256::digest(bytes));
    if let Some(sidecar) = &sidecar {
        let _ = std::fs::write(sidecar, format!("{stamp}\n{hash}"));
    }
    Ok(hash)
}

/// The digest a stored input path had, relative to `root`.
fn digest(root: &Path, path: &str) -> String {
    let path = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    match std::fs::read(&path) {
        Ok(bytes) => format!("{:x}", Sha256::digest(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => "missing".into(),
        Err(error) => format!("unreadable:{}", error.kind() as i32),
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).map_or_else(
        |_| path.to_string_lossy().replace('\\', "/"),
        |path| path.to_string_lossy().replace('\\', "/"),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::Cache;
    use crate::engine::{Outcome, Status};
    use crate::suite::Kind;

    fn outcome(id: &str, inputs: Vec<PathBuf>) -> Outcome {
        Outcome {
            id: id.into(),
            kind: Kind::Testharness,
            status: Status::Pass,
            message: None,
            subtests: Vec::new(),
            duration: Duration::ZERO,
            inputs,
        }
    }

    #[test]
    fn a_verdict_is_reused_until_an_input_changes() {
        let root = std::env::temp_dir().join(format!("wpt-harness-cache-{}", std::process::id()));
        std::fs::create_dir_all(root.join("case")).expect("root created");
        let test = root.join("case/test.html");
        let helper = root.join("case/helper.js");
        std::fs::write(&test, "<script src=helper.js></script>").expect("test written");
        std::fs::write(&helper, "var x = 1;").expect("helper written");

        let mut cache = Cache::empty("binary");
        cache.insert(
            &outcome("/case/test.html", vec![test, helper.clone()]),
            &root,
        );
        assert!(
            cache.lookup("/case/test.html", &root).is_some(),
            "unchanged inputs hit"
        );

        std::fs::write(&helper, "var x = 2;").expect("helper rewritten");
        assert!(
            cache.lookup("/case/test.html", &root).is_none(),
            "a changed helper invalidates"
        );

        std::fs::remove_file(&helper).expect("helper removed");
        assert!(
            cache.lookup("/case/test.html", &root).is_none(),
            "a missing input invalidates"
        );
        std::fs::remove_dir_all(&root).expect("root removed");
    }
}
