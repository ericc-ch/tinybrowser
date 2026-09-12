//! Leveled process logging: stderr console plus an async batched file sink.
//!
//! The model is inspired by Effect's `Logger`: a flat record with a level, a
//! target, and a message; independent sinks; a minimum-level threshold; and a
//! batched file writer. The surface is plain Rust: one process-global
//! [`Logger`] installed at startup and [`error!`], [`warn!`], [`info!`],
//! [`debug!`], and [`trace!`] macros.
//!
//! Nothing is logged until [`install`] is called, so libraries and tests stay
//! quiet by default. The console is **stderr only**: stdout carries command
//! results in the CLI and protocol JSON in the renderer.
//!
//! ```
//! logging::install(logging::Logger::new(
//!     logging::Config::new("cli").level(logging::Level::Debug),
//! ));
//! logging::info!(target: "example", "ready");
//! ```

mod file;
mod format;

use std::fmt;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Severity of one record, ordered from most to least severe.
///
/// A logger emits a record when its level is at or above the configured
/// minimum: with a minimum of [`Level::Info`], `Debug` and `Trace` are
/// filtered out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    /// A failure callers must act on.
    Error = 1,
    /// A recoverable problem worth attention.
    Warn = 2,
    /// Normal lifecycle events.
    Info = 3,
    /// Developer detail for diagnosing behavior.
    Debug = 4,
    /// Very detailed tracing.
    Trace = 5,
}

impl Level {
    /// Upper-case name as it appears in a log line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }

    /// Decodes a threshold from the atomic store.
    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Error,
            2 => Self::Warn,
            4 => Self::Debug,
            5 => Self::Trace,
            // 3 (Info) and any corrupted value.
            _ => Self::Info,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Level {
    type Err = ParseLevelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "error" => Ok(Self::Error),
            "warn" | "warning" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            _ => Err(ParseLevelError),
        }
    }
}

/// Error returned when a level name is not recognized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseLevelError;

impl fmt::Display for ParseLevelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unknown log level; expected error, warn, info, debug, or trace")
    }
}

impl std::error::Error for ParseLevelError {}

/// Process logger configuration, built with chained setters.
pub struct Config {
    process: &'static str,
    level: Level,
    console: bool,
    file: Option<PathBuf>,
}

impl Config {
    /// Console-on, no-file config for `process` at [`Level::Info`].
    #[must_use]
    pub fn new(process: &'static str) -> Self {
        Self {
            process,
            level: Level::Info,
            console: true,
            file: None,
        }
    }

    /// Sets the minimum level.
    #[must_use]
    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// Appends records to `path` from one background writer thread.
    #[must_use]
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file = Some(path.into());
        self
    }

    /// Enables or disables the stderr console (enabled by default).
    #[must_use]
    pub fn console(mut self, on: bool) -> Self {
        self.console = on;
        self
    }
}

/// One process logger: a threshold, a stderr console, and an optional file sink.
pub struct Logger {
    level: AtomicU8,
    process: &'static str,
    pid: u32,
    console: bool,
    file: Option<file::FileSink>,
    drops: AtomicU64,
}

impl Logger {
    /// Builds the logger.
    ///
    /// A file that cannot be opened is reported on stderr and skipped; logging
    /// never takes the process down.
    #[must_use]
    pub fn new(config: Config) -> Self {
        let file = config
            .file
            .and_then(|path| match file::FileSink::new(path.clone()) {
                Ok(sink) => Some(sink),
                Err(error) => {
                    eprintln!("logging: cannot open {}: {error}", path.display());
                    None
                }
            });
        Self {
            level: AtomicU8::new(config.level as u8),
            process: config.process,
            pid: std::process::id(),
            console: config.console,
            file,
            drops: AtomicU64::new(0),
        }
    }

    /// Current minimum level.
    #[must_use]
    pub fn level(&self) -> Level {
        Level::from_u8(self.level.load(Ordering::Relaxed))
    }

    /// Changes the minimum level.
    pub fn set_level(&self, level: Level) {
        self.level.store(level as u8, Ordering::Relaxed);
    }

    /// Whether a record at `level` passes the threshold.
    #[must_use]
    pub fn enabled(&self, level: Level) -> bool {
        (level as u8) <= self.level.load(Ordering::Relaxed)
    }

    /// Writes one record when `level` is enabled.
    pub fn log(&self, level: Level, target: &str, args: fmt::Arguments<'_>) {
        if !self.enabled(level) {
            return;
        }
        self.report_drops();
        let line = format::record(self.process, self.pid, level, target, args);
        if !self.emit(&line) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Forwards one already-formatted line (a renderer child's stderr).
    pub fn log_forwarded(&self, line: &str) {
        self.report_drops();
        if !self.emit(line.trim_end_matches(['\r', '\n'])) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Flushes pending records; `false` when the file writer did not catch up
    /// within its bound, or is gone. True when there is no file sink.
    pub fn flush(&self) -> bool {
        self.report_drops();
        self.file.as_ref().is_none_or(file::FileSink::flush)
    }

    /// Writes one line to the console and queues it for the file.
    ///
    /// Returns `false` when the file queue is full; console failure is
    /// deliberately ignored.
    fn emit(&self, line: &str) -> bool {
        if self.console {
            write_console(line);
        }
        let Some(file) = &self.file else {
            return true;
        };
        file.try_send(line)
    }

    /// Emits one warning for records the bounded file queue has dropped.
    ///
    /// When the warning itself cannot be queued, the count is restored so the
    /// next record reports the full total instead of just the warning.
    fn report_drops(&self) {
        let dropped = self.drops.swap(0, Ordering::Relaxed);
        if dropped == 0 {
            return;
        }
        let line = format::dropped(self.process, self.pid, dropped);
        if !self.emit(&line) {
            self.drops.fetch_add(dropped, Ordering::Relaxed);
        }
    }
}

/// Writes one line to stderr; failures are ignored by design.
fn write_console(line: &str) {
    let stderr = io::stderr();
    let mut out = stderr.lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.write_all(b"\n");
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Installs the process logger. A second install is ignored.
pub fn install(logger: Logger) {
    let _ = LOGGER.set(logger);
}

/// Whether the installed logger accepts `level`; `false` when none is installed.
#[must_use]
pub fn enabled(level: Level) -> bool {
    LOGGER.get().is_some_and(|logger| logger.enabled(level))
}

/// The installed logger's minimum level, or [`Level::Info`] when none is
/// installed.
#[must_use]
pub fn level() -> Level {
    LOGGER.get().map_or(Level::Info, Logger::level)
}

/// Writes one record through the installed logger; no-op when none is installed.
pub fn log(level: Level, target: &str, args: fmt::Arguments<'_>) {
    if let Some(logger) = LOGGER.get() {
        logger.log(level, target, args);
    }
}

/// Forwards an already-formatted line through the installed logger.
pub fn log_forwarded(line: &str) {
    if let Some(logger) = LOGGER.get() {
        logger.log_forwarded(line);
    }
}

/// Flushes the installed logger's pending records; `false` when its file
/// writer did not catch up in time. True when no logger is installed.
#[must_use]
pub fn flush() -> bool {
    LOGGER.get().is_none_or(Logger::flush)
}

/// Logs at [`Level::Error`] when the threshold enables it.
///
/// Arguments are evaluated only when the record is enabled.
#[macro_export]
macro_rules! error {
    (target: $target:expr, $($arg:tt)*) => {
        if $crate::enabled($crate::Level::Error) {
            $crate::log($crate::Level::Error, $target, format_args!($($arg)*))
        }
    };
    ($($arg:tt)*) => {
        if $crate::enabled($crate::Level::Error) {
            $crate::log($crate::Level::Error, module_path!(), format_args!($($arg)*))
        }
    };
}

/// Logs at [`Level::Warn`] when the threshold enables it.
///
/// Arguments are evaluated only when the record is enabled.
#[macro_export]
macro_rules! warn {
    (target: $target:expr, $($arg:tt)*) => {
        if $crate::enabled($crate::Level::Warn) {
            $crate::log($crate::Level::Warn, $target, format_args!($($arg)*))
        }
    };
    ($($arg:tt)*) => {
        if $crate::enabled($crate::Level::Warn) {
            $crate::log($crate::Level::Warn, module_path!(), format_args!($($arg)*))
        }
    };
}

/// Logs at [`Level::Info`] when the threshold enables it.
///
/// Arguments are evaluated only when the record is enabled.
#[macro_export]
macro_rules! info {
    (target: $target:expr, $($arg:tt)*) => {
        if $crate::enabled($crate::Level::Info) {
            $crate::log($crate::Level::Info, $target, format_args!($($arg)*))
        }
    };
    ($($arg:tt)*) => {
        if $crate::enabled($crate::Level::Info) {
            $crate::log($crate::Level::Info, module_path!(), format_args!($($arg)*))
        }
    };
}

/// Logs at [`Level::Debug`] when the threshold enables it.
///
/// Arguments are evaluated only when the record is enabled.
#[macro_export]
macro_rules! debug {
    (target: $target:expr, $($arg:tt)*) => {
        if $crate::enabled($crate::Level::Debug) {
            $crate::log($crate::Level::Debug, $target, format_args!($($arg)*))
        }
    };
    ($($arg:tt)*) => {
        if $crate::enabled($crate::Level::Debug) {
            $crate::log($crate::Level::Debug, module_path!(), format_args!($($arg)*))
        }
    };
}

/// Logs at [`Level::Trace`] when the threshold enables it.
///
/// Arguments are evaluated only when the record is enabled.
#[macro_export]
macro_rules! trace {
    (target: $target:expr, $($arg:tt)*) => {
        if $crate::enabled($crate::Level::Trace) {
            $crate::log($crate::Level::Trace, $target, format_args!($($arg)*))
        }
    };
    ($($arg:tt)*) => {
        if $crate::enabled($crate::Level::Trace) {
            $crate::log($crate::Level::Trace, module_path!(), format_args!($($arg)*))
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_order_from_most_to_least_severe() {
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
        assert!(Level::Debug < Level::Trace);
    }

    #[test]
    fn levels_parse_and_display() {
        assert_eq!("error".parse::<Level>(), Ok(Level::Error));
        assert_eq!("WARNING".parse::<Level>(), Ok(Level::Warn));
        assert_eq!("Info".parse::<Level>(), Ok(Level::Info));
        assert_eq!("debug".parse::<Level>(), Ok(Level::Debug));
        assert_eq!("trace".parse::<Level>(), Ok(Level::Trace));
        assert!("verbose".parse::<Level>().is_err());
        assert_eq!(Level::Warn.to_string(), "WARN");
    }

    #[test]
    fn threshold_filters_less_severe_records() {
        let logger = Logger::new(Config::new("test").level(Level::Info).console(false));
        assert!(logger.enabled(Level::Error));
        assert!(logger.enabled(Level::Info));
        assert!(!logger.enabled(Level::Debug));
        logger.set_level(Level::Trace);
        assert!(logger.enabled(Level::Trace));
    }

    #[test]
    fn unopenable_file_falls_back_to_console_only() {
        let logger = Logger::new(
            Config::new("test")
                .level(Level::Info)
                .console(false)
                .file("/proc/tinybrowser-cannot-exist/log"),
        );
        assert!(logger.file.is_none());
        assert!(logger.enabled(Level::Error));
    }

    #[test]
    fn disabled_macros_do_not_evaluate_arguments() {
        let mut evaluated = false;
        crate::info!(target: "test", "{}", {
            evaluated = true;
            "value"
        });
        assert!(!evaluated, "a filtered macro must not run its arguments");
    }

    #[test]
    fn console_only_logger_flushes_trivially() {
        let logger = Logger::new(Config::new("test").level(Level::Info).console(false));
        assert!(logger.flush());
    }

    #[test]
    fn a_full_file_queue_drops_instead_of_blocking() {
        let (sink, _never_read) = file::FileSink::stalled(2);
        let logger = Logger {
            level: AtomicU8::new(Level::Trace as u8),
            process: "test",
            pid: 1,
            console: false,
            file: Some(sink),
            drops: AtomicU64::new(0),
        };
        logger.log(Level::Info, "test", format_args!("one"));
        logger.log(Level::Info, "test", format_args!("two"));
        assert_eq!(logger.drops.load(Ordering::Relaxed), 0);
        logger.log(Level::Info, "test", format_args!("three"));
        assert_eq!(logger.drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_failed_drop_warning_keeps_the_count() {
        let (sink, _never_read) = file::FileSink::stalled(1);
        let logger = Logger {
            level: AtomicU8::new(Level::Trace as u8),
            process: "test",
            pid: 1,
            console: false,
            file: Some(sink),
            drops: AtomicU64::new(0),
        };
        logger.log(Level::Info, "test", format_args!("one"));
        logger.log(Level::Info, "test", format_args!("two"));
        assert_eq!(logger.drops.load(Ordering::Relaxed), 1);
        // The warning cannot be queued either; the pending count must survive.
        logger.log(Level::Info, "test", format_args!("three"));
        assert_eq!(logger.drops.load(Ordering::Relaxed), 2);
    }
}
