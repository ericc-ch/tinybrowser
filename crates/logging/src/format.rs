//! One-line record rendering and RFC 3339 timestamps.
//!
//! A record is `timestamp LEVEL target message`. The message is the last
//! field, so it may contain spaces; line breaks are escaped to keep one
//! record on one physical line.

use std::fmt::{self, Write as _};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Level;

/// Renders one record, ending in `\n`.
pub(crate) fn record(level: Level, target: &str, args: fmt::Arguments<'_>) -> String {
    let mut line = timestamp(SystemTime::now());
    line.push(' ');
    line.push_str(level.as_str());
    line.push(' ');
    push_escaped(&mut line, target);
    line.push(' ');
    let mut message = String::new();
    let _ = write!(message, "{args}");
    push_escaped(&mut line, &message);
    line.push('\n');
    line
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

/// Appends `value`, escaping line breaks so one record stays one line.
fn push_escaped(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
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
    fn records_are_one_plain_line() {
        let line = record(Level::Info, "browser::tab", format_args!("created tab"));
        assert_eq!(line.lines().count(), 1, "{line}");
        let line = line.trim_end_matches('\n');
        let mut fields = line.splitn(4, ' ');
        let timestamp = fields.next().expect("timestamp");
        assert!(timestamp.ends_with('Z'), "{timestamp}");
        assert_eq!(fields.next(), Some("INFO"));
        assert_eq!(fields.next(), Some("browser::tab"));
        assert_eq!(fields.next(), Some("created tab"));
    }

    #[test]
    fn newlines_in_messages_stay_on_one_line() {
        let line = record(Level::Error, "cli", format_args!("bad \"value\" a=b\nnext"));
        assert_eq!(line.lines().count(), 1, "{line}");
        assert!(line.contains("bad \"value\" a=b\\nnext"), "{line}");
    }

    #[test]
    fn newlines_in_targets_stay_on_one_line() {
        let line = record(Level::Info, "a\nb", format_args!("m"));
        assert_eq!(line.lines().count(), 1, "{line}");
        assert!(line.contains(" a\\nb m"), "{line}");
    }
}
