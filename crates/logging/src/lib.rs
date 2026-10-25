//! Process logging: a stderr console plus an optional background file sink.
//!
//! Nothing is logged until [`install`] is called, so libraries and tests stay
//! quiet by default. The console is **stderr only**: the CLI prints command
//! results to stdout, on platforms without a socket transport the renderer
//! protocol rides stdout, and on Unix a renderer's stdout is `/dev/null` (its
//! transport is the socket on stdin). stderr is the stream the daemon can
//! read, forward, and persist.
//!
//! One record is one line:
//!
//! ```text
//! 2026-10-05T00:00:00.123Z INFO browser::tab created tab
//! ```
//!
//! The file sink is best-effort and never blocks a logging thread. A full
//! queue drops lines. Release builds abort on panic (`panic = "abort"`), so
//! within the panic path the queue is bypassed: [`install`] also sets a panic
//! hook that appends the panic record straight to the log file and stderr.
//!
//! ```
//! logging::install(
//!     logging::Level::Debug,
//!     None,
//!     std::time::SystemTime::now,
//! )
//! .expect("logger");
//! logging::info!(target: "example", "ready");
//! ```

mod crash;
mod file;
mod format;

use std::fmt;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::SystemTime;

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

/// Why the process logger could not be installed.
#[derive(Debug)]
pub enum InstallError {
    /// A logger is already installed in this process.
    AlreadyInstalled,
    /// The log file could not be opened.
    Open {
        /// The path the logger tried to open.
        path: PathBuf,
        /// The operating-system failure.
        source: io::Error,
    },
    /// The log file opened, but its writer thread could not start.
    SpawnWriter(io::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInstalled => formatter.write_str("logger already installed"),
            Self::Open { path, .. } => {
                write!(formatter, "cannot open log file {}", path.display())
            }
            Self::SpawnWriter(_) => formatter.write_str("cannot start the log writer"),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open { source, .. } | Self::SpawnWriter(source) => Some(source),
            Self::AlreadyInstalled => None,
        }
    }
}

/// Installs the process logger: records at or above `level` go to stderr and,
/// when `file` is given, to that file from one background writer thread. Also
/// installs the panic hook, which writes its record synchronously instead of
/// through the queue.
///
/// `clock` supplies the wall time for every record, including the panic
/// record.
///
/// # Errors
/// Returns [`InstallError`] when this process already has a logger or when the
/// file sink cannot be opened or started. The composition root decides whether
/// that is fatal.
pub fn install(
    level: Level,
    file: Option<PathBuf>,
    clock: fn() -> SystemTime,
) -> Result<(), InstallError> {
    let _guard = INSTALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if LOGGER.get().is_some() {
        return Err(InstallError::AlreadyInstalled);
    }
    let logger = Logger::new(level, file, clock)?;
    LOGGER
        .set(logger)
        .map_err(|_| InstallError::AlreadyInstalled)?;
    crash::install(LOGGER.get().ok_or(InstallError::AlreadyInstalled)?);
    Ok(())
}

struct Logger {
    level: Level,
    clock: fn() -> SystemTime,
    file: Option<file::FileSink>,
}

impl Logger {
    fn new(
        level: Level,
        path: Option<PathBuf>,
        clock: fn() -> SystemTime,
    ) -> Result<Self, InstallError> {
        let file = path.map(file::FileSink::new).transpose()?;
        Ok(Self { level, clock, file })
    }

    /// Whether a record at `level` passes the threshold.
    fn enabled(&self, level: Level) -> bool {
        level <= self.level
    }

    /// Writes one record when `level` is enabled.
    fn log(&self, level: Level, target: &str, args: fmt::Arguments<'_>) {
        if self.enabled(level) {
            self.emit(format::record((self.clock)(), level, target, args));
        }
    }

    /// Forwards one already-formatted line (a renderer child's stderr).
    fn log_forwarded(&self, line: &str) {
        self.emit(format::forwarded(line));
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
static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let logger = Logger::new(Level::Info, None, SystemTime::now).expect("logger");
        assert!(logger.enabled(Level::Error));
        assert!(logger.enabled(Level::Info));
        assert!(!logger.enabled(Level::Debug));
    }

    #[test]
    fn unopenable_file_reports_its_path() {
        let path = PathBuf::from("/proc/tinybrowser-cannot-exist/log");
        let logger = Logger::new(Level::Info, Some(path.clone()), SystemTime::now);
        assert!(matches!(
            logger,
            Err(InstallError::Open { path: failed, .. }) if failed == path
        ));
    }

    #[test]
    fn records_stamp_with_the_injected_clock() {
        let dir =
            std::env::temp_dir().join(format!("tinybrowser-log-clock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("log");
        let logger = Logger::new(Level::Info, Some(path.clone()), || SystemTime::UNIX_EPOCH)
            .expect("logger");
        logger.log(Level::Info, "test", format_args!("stamped"));
        assert!(logger.flush());
        let contents = std::fs::read_to_string(&path).expect("read");
        assert!(
            contents.contains("1970-01-01T00:00:00.000Z INFO test stamped"),
            "{contents}"
        );
        let _ = std::fs::remove_dir_all(dir);
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
}
