//! The `input` element's type state, value modes, and value sanitization.
//!
//! Blitz stores the current value as a dirty value slot plus the `value`
//! content attribute as the default
//! (<https://html.spec.whatwg.org/multipage/input.html#concept-fe-value>).
//! The sanitization algorithms live here so the IDL getter, form submission,
//! and type changes all read one current value
//! (<https://html.spec.whatwg.org/multipage/input.html#the-input-element>).

use std::fmt::Write as _;

use blitz_dom::{BaseDocument, NodeData};

use crate::dom::tree::{BlitzId, attr};

/// One state of the `type` content attribute. An unknown keyword uses the
/// Text state (<https://html.spec.whatwg.org/multipage/input.html#attr-input-type>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputType {
    Hidden,
    Text,
    Search,
    Tel,
    Url,
    Email,
    Password,
    Date,
    Month,
    Week,
    Time,
    DatetimeLocal,
    Number,
    Range,
    Color,
    Checkbox,
    Radio,
    File,
    Submit,
    Image,
    Reset,
    Button,
}

/// The mode of the `value` IDL attribute in a state
/// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ValueMode {
    /// The current value is exposed and settable.
    Current,
    /// The value content attribute is exposed. Its missing default is empty.
    Default,
    /// The value content attribute is exposed. Its missing default is `"on"`.
    DefaultOn,
    /// The selected file list is exposed, never the value attribute.
    Filename,
}

impl InputType {
    /// Parses a `type` content attribute value; `None` and unknown keywords
    /// use the Text state.
    pub(crate) fn from_attribute(value: Option<&str>) -> Self {
        let value = value.unwrap_or("").trim().to_ascii_lowercase();
        match value.as_str() {
            "hidden" => Self::Hidden,
            "search" => Self::Search,
            "tel" => Self::Tel,
            "url" => Self::Url,
            "email" => Self::Email,
            "password" => Self::Password,
            "date" => Self::Date,
            "month" => Self::Month,
            "week" => Self::Week,
            "time" => Self::Time,
            "datetime-local" => Self::DatetimeLocal,
            "number" => Self::Number,
            "range" => Self::Range,
            "color" => Self::Color,
            "checkbox" => Self::Checkbox,
            "radio" => Self::Radio,
            "file" => Self::File,
            "submit" => Self::Submit,
            "image" => Self::Image,
            "reset" => Self::Reset,
            "button" => Self::Button,
            _ => Self::Text,
        }
    }

    /// Whether the text selection APIs apply in this state
    /// (<https://html.spec.whatwg.org/multipage/input.html#do-not-apply>).
    pub(crate) fn selection_applies(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Search | Self::Tel | Self::Url | Self::Password
        )
    }

    pub(crate) fn value_mode(self) -> ValueMode {
        match self {
            Self::Checkbox | Self::Radio => ValueMode::DefaultOn,
            Self::Submit | Self::Image | Self::Reset | Self::Button => ValueMode::Default,
            Self::File => ValueMode::Filename,
            Self::Hidden
            | Self::Text
            | Self::Search
            | Self::Tel
            | Self::Url
            | Self::Email
            | Self::Password
            | Self::Date
            | Self::Month
            | Self::Week
            | Self::Time
            | Self::DatetimeLocal
            | Self::Number
            | Self::Range
            | Self::Color => ValueMode::Current,
        }
    }
}

/// The current type state of `node`, or `None` when it is not an HTML
/// `input` element.
pub(crate) fn input_type(base: &BaseDocument, node: BlitzId) -> Option<InputType> {
    let element = base.get_node(node)?.data.downcast_element()?;
    if element.name.ns != crate::dom::tree::html_namespace()
        || element.name.local.as_str() != "input"
    {
        return None;
    }
    Some(InputType::from_attribute(attr(base, node, "type")))
}

/// The raw current value: the dirty value when set, otherwise the `value`
/// content attribute
/// (<https://html.spec.whatwg.org/multipage/input.html#concept-fe-value>).
pub(crate) fn raw_value(base: &BaseDocument, node: BlitzId) -> String {
    let element = base
        .get_node(node)
        .and_then(|target| target.data.downcast_element());
    match element {
        Some(element) if element.form_state.dirty_value => element.form_state.value.clone(),
        _ => attr(base, node, "value").unwrap_or("").to_owned(),
    }
}

/// The current value of an input: the raw current value after the state's
/// value sanitization algorithm
/// (<https://html.spec.whatwg.org/multipage/input.html#concept-fe-value>).
pub(crate) fn current_value(base: &BaseDocument, node: BlitzId) -> String {
    let Some(kind) = input_type(base, node) else {
        return String::new();
    };
    if kind == InputType::File {
        return String::new();
    }
    let raw = raw_value(base, node);
    if kind.value_mode() == ValueMode::DefaultOn
        && raw.is_empty()
        && attr(base, node, "value").is_none()
    {
        return "on".to_owned();
    }
    sanitize(base, node, kind, &raw)
}

/// Applies a state's value sanitization algorithm
/// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element>).
pub(crate) fn sanitize(base: &BaseDocument, node: BlitzId, kind: InputType, raw: &str) -> String {
    match kind {
        InputType::Hidden
        | InputType::Checkbox
        | InputType::Radio
        | InputType::Submit
        | InputType::Image
        | InputType::Reset
        | InputType::Button => raw.to_owned(),
        InputType::Text | InputType::Search | InputType::Tel | InputType::Password => {
            strip_newlines(raw)
        }
        InputType::Url => strip_newlines(raw).trim_ascii_whitespace().to_owned(),
        InputType::Email => {
            let stripped = strip_newlines(raw);
            if attr(base, node, "multiple").is_some() {
                stripped
                    .split(',')
                    .map(str::trim_ascii_whitespace)
                    .collect::<Vec<_>>()
                    .join(",")
            } else {
                stripped.trim_ascii_whitespace().to_owned()
            }
        }
        InputType::Date => {
            if valid_date(raw) {
                raw.to_owned()
            } else {
                String::new()
            }
        }
        InputType::Month => {
            if valid_month(raw) {
                raw.to_owned()
            } else {
                String::new()
            }
        }
        InputType::Week => {
            if valid_week(raw) {
                raw.to_owned()
            } else {
                String::new()
            }
        }
        InputType::Time => {
            if valid_time(raw).is_some() {
                raw.to_owned()
            } else {
                String::new()
            }
        }
        InputType::DatetimeLocal => valid_local_datetime(raw)
            .map(|(date, time)| format!("{date}T{}", normalized_time(time)))
            .unwrap_or_default(),
        InputType::Number => {
            if valid_floating_point(raw) {
                raw.to_owned()
            } else {
                String::new()
            }
        }
        InputType::Range => sanitize_range(base, node, raw),
        InputType::Color => sanitize_color(raw),
        InputType::File => String::new(),
    }
}

fn strip_newlines(value: &str) -> String {
    value
        .chars()
        .filter(|character| *character != '\n' && *character != '\r')
        .collect()
}

trait TrimAsciiWhitespace {
    fn trim_ascii_whitespace(&self) -> &str;
}

impl TrimAsciiWhitespace for str {
    fn trim_ascii_whitespace(&self) -> &str {
        self.trim_matches(|character: char| character.is_ascii_whitespace())
    }
}

/// The range state's default value, or the value clamped into the accepted
/// interval (<https://html.spec.whatwg.org/multipage/input.html#range-state-(type=range)>).
fn sanitize_range(base: &BaseDocument, node: BlitzId, raw: &str) -> String {
    let minimum = attr(base, node, "min")
        .filter(|value| valid_floating_point(value))
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    let maximum = attr(base, node, "max")
        .filter(|value| valid_floating_point(value))
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(100.0);
    let default = if maximum < minimum {
        minimum
    } else {
        minimum + (maximum - minimum) / 2.0
    };
    let mut value = if valid_floating_point(raw) {
        raw.parse::<f64>().unwrap_or(default)
    } else {
        default
    };
    if value < minimum {
        value = minimum;
    } else if value > maximum && maximum >= minimum {
        value = maximum;
    }
    // A range value is always step-aligned: round to the nearest allowed
    // value, with a tie rounded up
    // (<https://html.spec.whatwg.org/multipage/input.html#range-state-(type=range)>).
    let step = attr(base, node, "step");
    if step.is_none_or(|step| !step.trim().eq_ignore_ascii_case("any")) {
        let step = step
            .filter(|step| valid_floating_point(step))
            .and_then(|step| step.parse::<f64>().ok())
            .filter(|step| *step > 0.0)
            .unwrap_or(1.0);
        let base_value = attr(base, node, "value")
            .filter(|value| valid_floating_point(value))
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0);
        let step_base = if attr(base, node, "min").is_some_and(valid_floating_point) {
            minimum
        } else {
            base_value
        };
        value = step_base + ((value - step_base) / step + 0.5).floor() * step;
        value = value.clamp(minimum.min(maximum), maximum.max(minimum));
    }
    format_f64(value)
}

/// The color state's value: a CSS color serialized as an opaque simple color,
/// or `#000000` when parsing fails
/// (<https://html.spec.whatwg.org/multipage/input.html#color-state-(type=color)>).
fn sanitize_color(raw: &str) -> String {
    match crate::dom::css_color::parse_exact(raw) {
        Some(color) if color.bytes[3] == 255 => {
            let [red, green, blue, _] = color.bytes;
            format!("#{red:02x}{green:02x}{blue:02x}")
        }
        _ => "#000000".to_owned(),
    }
}

/// The shortest decimal representation that round-trips
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#best-representation-of-the-number-as-a-floating-point-number>).
fn format_f64(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    // Step alignment multiplies binary fractions, so 6.8 can arrive as
    // 6.800000000000001. Twelve decimals is far below the precision the
    // range state exposes.
    let formatted = format!("{value:.12}");
    let formatted = formatted.trim_end_matches('0').trim_end_matches('.');
    formatted.to_owned()
}

/// A valid floating-point number: an optional `-`, digits with an optional
/// fraction, and an optional exponent, with a finite value
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-floating-point-number>).
pub(crate) fn valid_floating_point(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut position = 0;
    if bytes.first() == Some(&b'-') {
        position += 1;
    }
    let integer_start = position;
    while bytes.get(position).is_some_and(u8::is_ascii_digit) {
        position += 1;
    }
    let integer_digits = position - integer_start;
    let mut fraction_digits = 0;
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        let fraction_start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        fraction_digits = position - fraction_start;
        if fraction_digits == 0 {
            return false;
        }
    }
    if integer_digits == 0 && fraction_digits == 0 {
        return false;
    }
    if bytes
        .get(position)
        .is_some_and(|byte| *byte == b'e' || *byte == b'E')
    {
        position += 1;
        if bytes
            .get(position)
            .is_some_and(|byte| *byte == b'+' || *byte == b'-')
        {
            position += 1;
        }
        let exponent_start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if position == exponent_start {
            return false;
        }
    }
    if position != bytes.len() {
        return false;
    }
    value.parse::<f64>().is_ok_and(f64::is_finite)
}

/// A valid date string: year > 0, a real month, and a day in that month
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-date-string>).
pub(crate) fn valid_date(value: &str) -> bool {
    parse_date(value).is_some()
}

fn parse_date(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.split('-');
    let year = parse_year(parts.next()?)?;
    let month = parse_two_digits(parts.next()?)?;
    let day = parse_two_digits(parts.next()?)?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    let max_day = days_in_month(year, month);
    (1..=max_day).contains(&day).then_some((year, month, day))
}

fn parse_year(value: &str) -> Option<u32> {
    if value.len() < 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u32>().ok().filter(|year| *year > 0)
}

fn parse_two_digits(value: &str) -> Option<u32> {
    if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u32>().ok()
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// A valid month string: year > 0 and a month in 1..=12
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-month-string>).
pub(crate) fn valid_month(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(_year) = parts.next().and_then(parse_year) else {
        return false;
    };
    let Some(month) = parts.next().and_then(parse_two_digits) else {
        return false;
    };
    parts.next().is_none() && (1..=12).contains(&month)
}

/// A valid week string: year > 0, an uppercase `W`, and a week present in
/// that week-year
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-week-string>).
pub(crate) fn valid_week(value: &str) -> bool {
    let Some((year, rest)) = value.split_once('-') else {
        return false;
    };
    let Some(week) = rest.strip_prefix('W').and_then(parse_two_digits) else {
        return false;
    };
    let Some(year) = parse_year(year) else {
        return false;
    };
    (1..=weeks_in_year(year)).contains(&week)
}

/// The ISO week number of the last day of `year` (December 28 is always in
/// the last week).
fn weeks_in_year(year: u32) -> u32 {
    iso_week(year, 12, 28)
}

fn iso_week(year: u32, month: u32, day: u32) -> u32 {
    let ordinal = ordinal_day(year, month, day);
    let weekday = weekday_from_days(days_from_civil(year, month, day));
    (ordinal - weekday + 10) / 7
}

fn ordinal_day(year: u32, month: u32, day: u32) -> u32 {
    (1..month)
        .map(|earlier| days_in_month(year, earlier))
        .sum::<u32>()
        + day
}

/// Days since 1970-01-01 (Howard Hinnant's civil calendar algorithm).
fn days_from_civil(year: u32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Monday is 0 ... Sunday is 6; 1970-01-01 was a Thursday.
fn weekday_from_days(days: i64) -> u32 {
    u32::try_from((days + 3).rem_euclid(7)).unwrap_or(0)
}

/// A parsed time: hour, minute, second, and milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Time {
    pub(crate) hour: u32,
    pub(crate) minute: u32,
    pub(crate) second: u32,
    pub(crate) millis: u32,
}

/// A valid time string: `hh:mm`, optional `:ss`, and an optional fraction of
/// one to three digits
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-time-string>).
pub(crate) fn valid_time(value: &str) -> Option<Time> {
    let (clock, fraction) = match value.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (value, None),
    };
    let mut parts = clock.split(':');
    let hour = parse_two_digits(parts.next()?)?;
    let minute = parse_two_digits(parts.next()?)?;
    let second = match parts.next() {
        Some(second) => parse_two_digits(second)?,
        None => 0,
    };
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let millis = match fraction {
        Some(fraction) => {
            if fraction.is_empty()
                || fraction.len() > 3
                || !fraction.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            let digits = fraction.parse::<u32>().ok()?;
            digits * 10u32.pow(u32::try_from(3 - fraction.len()).ok()?)
        }
        None => 0,
    };
    Some(Time {
        hour,
        minute,
        second,
        millis,
    })
}

/// A valid local date and time string, split into its date and time
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-local-date-and-time-string>).
pub(crate) fn valid_local_datetime(value: &str) -> Option<(&str, Time)> {
    let separator = value.find(['T', ' '])?;
    let (date, rest) = value.split_at(separator);
    let time = valid_time(&rest[1..])?;
    valid_date(date).then_some((date, time))
}

/// The shortest time string for a parsed time: seconds and fractions are
/// omitted when zero, and trailing fraction zeros are dropped
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-normalized-local-date-and-time-string>).
pub(crate) fn normalized_time(time: Time) -> String {
    let mut normalized = format!("{:02}:{:02}", time.hour, time.minute);
    if time.second != 0 || time.millis != 0 {
        let _ = write!(normalized, ":{:02}", time.second);
    }
    if time.millis != 0 {
        let fraction = format!("{:03}", time.millis);
        normalized.push('.');
        normalized.push_str(fraction.trim_end_matches('0'));
    }
    normalized
}

/// Replaces a text control's raw value with its API value: the raw value
/// with CRLF and CR normalized to LF
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-textarea-api-value>).
pub(crate) fn normalize_newlines(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    normalized
}

/// A node's child text content: the data of its Text children in tree order
/// (<https://dom.spec.whatwg.org/#concept-child-text-content>).
pub(crate) fn child_text_content(base: &BaseDocument, node: BlitzId) -> String {
    let mut text = String::new();
    let Some(target) = base.get_node(node) else {
        return text;
    };
    for child in &target.children {
        match base.get_node(*child).map(|child| &child.data) {
            Some(NodeData::Text(data)) => text.push_str(&data.content),
            Some(NodeData::CDataSection { contents }) => text.push_str(contents),
            _ => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{
        Time, normalize_newlines, normalized_time, valid_date, valid_floating_point,
        valid_local_datetime, valid_month, valid_time, valid_week,
    };

    #[test]
    fn floating_point_grammar() {
        assert!(valid_floating_point("-0"));
        for valid in ["1", ".1", "1.5", "1e2", "1E+2", "-1e-2", "9007199254740993"] {
            assert!(valid_floating_point(valid), "{valid}");
        }
        for invalid in ["", "+1", "1.", "1e", " 1", "1 ", "Infinity", "NaN", "2e308"] {
            assert!(!valid_floating_point(invalid), "{invalid}");
        }
    }

    #[test]
    fn date_and_time_validity() {
        assert!(valid_date("2019-12-10"));
        assert!(valid_date("2016-02-29"));
        for invalid in [
            "0000-12-10",
            "2019-00-12",
            "2019-13-10",
            "2019-02-29",
            "19-01-01",
        ] {
            assert!(!valid_date(invalid), "{invalid}");
        }
        assert!(valid_month("2019-12"));
        assert!(!valid_month("0000-12"));
        assert!(valid_week("2020-W53"));
        assert!(!valid_week("2019-W53"));
        assert!(!valid_week("2019-w01"));
        assert_eq!(
            valid_time("23:59:59.5"),
            Some(Time {
                hour: 23,
                minute: 59,
                second: 59,
                millis: 500
            })
        );
        assert_eq!(valid_time("24:00"), None);
        assert_eq!(valid_time("00:60"), None);
        assert_eq!(normalized_time(valid_time("12:30").unwrap()), "12:30");
        assert_eq!(
            normalized_time(valid_time("12:30:01.500").unwrap()),
            "12:30:01.5"
        );
        assert!(valid_local_datetime("2019-12-10 00:00").is_some());
        assert!(valid_local_datetime("2019-12-10T24:00").is_none());
    }

    #[test]
    fn newline_normalization() {
        assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(normalize_newlines("a\nb"), "a\nb");
    }
}
