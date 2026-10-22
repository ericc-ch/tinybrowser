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

/// Where the installed logger writes, so panic records land in the same file.
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Installs the process panic hook. A second call is ignored by the caller's
/// one-time logger install.
pub(crate) fn install(path: Option<PathBuf>) {
    if let Some(path) = path {
        let _ = LOG_PATH.set(path);
    }
    std::panic::set_hook(Box::new(record));
}

/// Renders and writes one panic record.
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
    let line = format_line(&location, message);
    crate::write_console(&line);
    if let Some(path) = LOG_PATH.get() {
        append(path, &line);
    }
}

/// The log record for one panic, ending in `\n`.
fn format_line(location: &str, message: &str) -> String {
    crate::format::record(
        crate::Level::Error,
        "panic",
        format_args!("{location} {message}"),
    )
}

/// Appends one record through a fresh handle: the writer thread rotates the
/// file to `<file>.1`, so a cached handle could point at the rotated inode.
/// Opens through the sink's own `open`, so a hook-created log gets the same
/// 0600 file and 0700 parent as the sink's.
fn append(path: &std::path::Path, line: &str) {
    if let Ok(mut file) = crate::file::open(path) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::append;

    #[test]
    fn installed_hook_records_a_panic_in_the_log_file() {
        // Process-global state (logger + panic hook) is safe here because the
        // workspace pins RUST_TEST_THREADS=1 and this is the only test that
        // installs the logger.
        let dir = std::env::temp_dir().join(format!(
            "tinybrowser-panic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("log");
        crate::install(crate::Level::Error, Some(path.clone()));
        let caught = std::panic::catch_unwind(|| panic!("hook probe {}", 7));
        assert!(caught.is_err());
        let contents = std::fs::read_to_string(&path).expect("read");
        assert!(contents.contains(" ERROR panic "), "{contents}");
        assert!(contents.contains("hook probe 7"), "{contents}");
        assert!(contents.contains("crash.rs:"), "{contents}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn appends_to_an_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "tinybrowser-crash-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("log");
        std::fs::write(&path, "existing\n").expect("seed");
        append(&path, "crash line\n");
        append(&path, "second line\n");
        let contents = std::fs::read_to_string(&path).expect("read");
        assert_eq!(contents, "existing\ncrash line\nsecond line\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
