//! Process logging: a stderr console plus an optional background file sink.
//!
//! Nothing is logged until [`install`] is called, so libraries and tests stay
//! quiet by default. The console is **stderr only**: stdout carries command
//! results in the CLI and protocol JSON in the renderer.
//!
//! One record is one line:
//!
//! ```text
//! 2026-10-05T00:00:00.123Z INFO browser::tab created tab
//! ```
//!
//! The file sink is best-effort and never blocks a logging thread. A full
//! queue drops lines, and release builds abort on panic (`panic = "abort"`),
//! so whatever is still queued at that point is lost.
//!
//! ```
//! logging::install(logging::Level::Debug, None);
//! logging::info!(target: "example", "ready");
//! ```

mod file;
mod format;

use std::fmt;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;

/// Severity of one record, ordered from most to least severe.
///
/// A logger emits a record when its level is at or above the configured
/// minimum: with a minimum of [`Level::Info`], `Debug` and `Trace` are
/// filtered out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// A failure callers must act on.
    Error,
    /// A recoverable problem worth attention.
    Warn,
    /// Normal lifecycle events.
    Info,
    /// Developer detail for diagnosing behavior.
    Debug,
    /// Very detailed tracing.
    Trace,
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

/// Installs the process logger: records at or above `level` go to stderr and,
/// when `file` is given, to that file from one background writer thread.
///
/// A file that cannot be opened is reported on stderr and skipped; logging
/// never takes the process down. A second install is ignored.
pub fn install(level: Level, file: Option<PathBuf>) {
    let _ = LOGGER.set(Logger::new(level, file));
}

struct Logger {
    level: Level,
    file: Option<file::FileSink>,
}

impl Logger {
    fn new(level: Level, path: Option<PathBuf>) -> Self {
        let file = path.and_then(|path| match file::FileSink::new(path.clone()) {
            Ok(sink) => Some(sink),
            Err(error) => {
                eprintln!("logging: cannot open {}: {error}", path.display());
                None
            }
        });
        Self { level, file }
    }

    /// Whether a record at `level` passes the threshold.
    fn enabled(&self, level: Level) -> bool {
        level <= self.level
    }

    /// Writes one record when `level` is enabled.
    fn log(&self, level: Level, target: &str, args: fmt::Arguments<'_>) {
        if self.enabled(level) {
            self.emit(format::record(level, target, args));
        }
    }

    /// Forwards one already-formatted line (a renderer child's stderr).
    fn log_forwarded(&self, line: &str) {
        let mut line = line.trim_end_matches(['\r', '\n']).to_owned();
        line.push('\n');
        self.emit(line);
    }

    /// Writes one full line to the console and queues it for the file.
    fn emit(&self, line: String) {
        write_console(&line);
        if let Some(file) = &self.file {
            file.send(line);
        }
    }

    /// Flushes pending records; `false` when the file writer did not catch up
    /// within its bound, or is gone. True when there is no file sink.
    fn flush(&self) -> bool {
        self.file.as_ref().is_none_or(file::FileSink::flush)
    }
}

/// Writes one line to stderr; failures are ignored by design.
fn write_console(line: &str) {
    let stderr = io::stderr();
    let mut out = stderr.lock();
    let _ = out.write_all(line.as_bytes());
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Whether the installed logger accepts `level`; `false` when none is installed.
#[must_use]
pub fn enabled(level: Level) -> bool {
    LOGGER.get().is_some_and(|logger| logger.enabled(level))
}

/// The installed logger's minimum level, or [`Level::Info`] when none is
/// installed.
#[must_use]
pub fn level() -> Level {
    LOGGER.get().map_or(Level::Info, |logger| logger.level)
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
        let logger = Logger::new(Level::Info, None);
        assert!(logger.enabled(Level::Error));
        assert!(logger.enabled(Level::Info));
        assert!(!logger.enabled(Level::Debug));
    }

    #[test]
    fn unopenable_file_falls_back_to_console_only() {
        let logger = Logger::new(
            Level::Info,
            Some(PathBuf::from("/proc/tinybrowser-cannot-exist/log")),
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
        let logger = Logger::new(Level::Info, None);
        assert!(logger.flush());
    }
}
