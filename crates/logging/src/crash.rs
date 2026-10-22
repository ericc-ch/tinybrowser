//! Synchronous panic records.
//!
//! Release builds use `panic = "abort"`: the panic hook runs, then the process
//! aborts. The regular file sink is a background queue that dies with the
//! process, so a panic record written through it would be lost. This hook
//! bypasses the queue and appends straight to the log file, then stderr (the
//! stream the daemon forwards from a renderer).
//!
//! The hook must not panic: every step is best-effort.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

pub(crate) fn install(path: Option<PathBuf>) {
    if let Some(path) = path {
        let _ = LOG_PATH.set(path);
    }
    std::panic::set_hook(Box::new(record));
}

fn record(info: &std::panic::PanicHookInfo<'_>) {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload");
    let location = info.location().map_or_else(
        || "unknown location".to_owned(),
        |location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        },
    );
    let line = crate::format::record(
        std::time::SystemTime::now(),
        crate::Level::Error,
        "panic",
        format_args!("{location} {message}"),
    );
    crate::write_console(&line);
    if let Some(path) = LOG_PATH.get() {
        append(path, &line);
    }
}

/// Opens through the sink's own `open`, so a hook-created log gets the same
/// 0600 file and 0700 parent as the sink's. A fresh handle also follows the
/// writer thread's `<file>.1` rotation instead of a cached inode.
fn append(path: &std::path::Path, line: &str) {
    if let Ok(mut file) = crate::file::open(path) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::append;
    use std::path::{Path, PathBuf};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tinybrowser-{prefix}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn appends_to_an_existing_file() {
        let dir = TempDir::new("crash");
        let path = dir.path().join("log");
        std::fs::write(&path, "existing\n").expect("seed");
        append(&path, "crash line\n");
        append(&path, "second line\n");
        let contents = std::fs::read_to_string(&path).expect("read");
        assert_eq!(contents, "existing\ncrash line\nsecond line\n");
    }
}
