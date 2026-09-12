//! logfmt line rendering and RFC 3339 timestamps.
//!
//! One record is always one line: whitespace in a value is either quoted or
//! escaped, so forwarded renderer lines and `grep`/`tail` both stay usable.

use std::fmt::{self, Write as _};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Level;

/// Renders one logfmt record.
pub(crate) fn record(
    process: &str,
    pid: u32,
    level: Level,
    target: &str,
    args: fmt::Arguments<'_>,
) -> String {
    let mut line = String::with_capacity(160);
    let _ = write!(line, "timestamp={}", timestamp(SystemTime::now()));
    push_field(&mut line, "level", level.as_str());
    push_field(&mut line, "process", process);
    let _ = write!(line, " pid={pid}");
    push_field(&mut line, "target", target);
    push_message(&mut line, args);
    line
}

/// Renders the warning emitted after [`crate::Logger`] dropped records.
pub(crate) fn dropped(process: &str, pid: u32, count: u64) -> String {
    record(
        process,
        pid,
        Level::Warn,
        "logging",
        format_args!("dropped {count} record(s): file writer is behind"),
    )
}

/// RFC 3339 UTC with milliseconds, for example `2026-09-12T00:00:00.123Z`.
pub(crate) fn timestamp(time: SystemTime) -> String {
    // Pre-epoch clocks clamp to the epoch instead of rendering a negative
    // year; system clocks before 1970 are not a case worth carrying.
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let millis = since_epoch.subsec_millis();
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Days since the Unix epoch to a proleptic Gregorian date.
///
/// Howard Hinnant's `civil_from_days`, shifted for the Unix epoch.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Appends ` name=value`, quoting and escaping when needed.
fn push_field(out: &mut String, name: &str, value: &str) {
    out.push(' ');
    out.push_str(name);
    out.push('=');
    if needs_quote(value) {
        push_quoted(out, value);
    } else {
        out.push_str(value);
    }
}

/// Appends ` message=...`, escaping newlines to keep the one-line invariant.
fn push_message(out: &mut String, args: fmt::Arguments<'_>) {
    let mut message = String::new();
    let _ = write!(message, "{args}");
    out.push_str(" message=");
    if needs_quote(&message) {
        push_quoted(out, &message);
    } else {
        out.push_str(&message);
    }
}

fn needs_quote(value: &str) -> bool {
    value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || c == '"' || c == '=')
}

/// Appends a quoted value, escaping quotes, backslashes, and line breaks so
/// one record always occupies one physical line.
fn push_quoted(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_rfc3339_utc() {
        let epoch = SystemTime::UNIX_EPOCH;
        assert_eq!(timestamp(epoch), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            timestamp(epoch + Duration::from_millis(1_789_171_200_123)),
            "2026-09-12T00:00:00.123Z"
        );
    }

    #[test]
    fn plain_records_are_one_logfmt_line() {
        let line = record(
            "daemon",
            42,
            Level::Info,
            "browser::tab",
            format_args!("created tab"),
        );
        let (timestamp, rest) = line.split_once(' ').expect("timestamp field");
        assert!(timestamp.starts_with("timestamp="), "{timestamp}");
        assert!(timestamp.ends_with('Z'), "{timestamp}");
        assert_eq!(
            rest,
            "level=INFO process=daemon pid=42 target=browser::tab message=\"created tab\""
        );
    }

    #[test]
    fn quotes_equals_and_newlines_stay_on_one_line() {
        let line = record(
            "cli",
            7,
            Level::Error,
            "cli",
            format_args!("bad \"value\" a=b\nnext"),
        );
        assert_eq!(line.lines().count(), 1, "{line}");
        let (_, rest) = line.split_once(' ').expect("timestamp field");
        assert_eq!(
            rest,
            "level=ERROR process=cli pid=7 target=cli message=\"bad \\\"value\\\" a=b\\nnext\""
        );
    }

    #[test]
    fn newlines_in_targets_stay_on_one_line() {
        let line = record("p", 1, Level::Info, "a\nb", format_args!("m"));
        assert_eq!(line.lines().count(), 1, "{line}");
        assert!(line.contains("target=\"a\\nb\""), "{line}");
    }

    #[test]
    fn drop_warning_is_a_warn_record() {
        let line = dropped("daemon", 3, 9);
        assert!(line.contains(" level=WARN "), "{line}");
        assert!(line.contains(" target=logging "), "{line}");
        assert!(line.contains("dropped 9 record(s)"), "{line}");
    }
}
