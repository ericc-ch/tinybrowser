//! Microsyntax validators backing the HTML input value sanitization
//! algorithms.
//!
//! <https://html.spec.whatwg.org/multipage/input.html#value-sanitization-algorithm>
//! <https://html.spec.whatwg.org/multipage/common-microsyntaxes.html>

/// Consumes `min..=max` leading ASCII digits, returning the parsed value and
/// the remaining input. Fails when the count is out of range or the digits do
/// not fit a `u32`.
fn take_digits(value: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let count = value.bytes().take_while(u8::is_ascii_digit).count();
    if count < min || count > max {
        return None;
    }
    let (digits, rest) = value.split_at(count);
    Some((digits.parse().ok()?, rest))
}

/// Whether `year` is a leap year in the proleptic Gregorian calendar.
fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// The number of days in `month` (1-12) of `year`.
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Whether `value` is a
/// [valid floating-point number](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-floating-point-number):
/// an optional sign, one or more digits with an optional fraction, and an
/// optional exponent, with no surrounding whitespace.
#[must_use]
pub fn is_valid_floating_point(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    if bytes.first() == Some(&b'-') {
        index += 1;
    }
    let integer_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let integer_digits = index - integer_start;
    let mut fraction_digits = 0;
    let mut had_dot = false;
    if bytes.get(index) == Some(&b'.') {
        had_dot = true;
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        fraction_digits = index - fraction_start;
    }
    if integer_digits == 0 && fraction_digits == 0 {
        return false;
    }
    // A dot must be followed by at least one digit (`1.` is not valid).
    if had_dot && fraction_digits == 0 {
        return false;
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }
    index == bytes.len()
}

/// Whether `value` is a
/// [valid month string](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-month-string)
/// (`YYYY-MM`).
#[must_use]
pub fn is_valid_month(value: &str) -> bool {
    let Some((year, rest)) = take_digits(value, 4, 6) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix('-') else {
        return false;
    };
    let Some((month, rest)) = take_digits(rest, 2, 2) else {
        return false;
    };
    rest.is_empty() && year > 0 && (1..=12).contains(&month)
}

/// Whether `value` is a
/// [valid date string](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-date-string)
/// (`YYYY-MM-DD`) with a day that exists in its month.
#[must_use]
pub fn is_valid_date(value: &str) -> bool {
    let Some((year, rest)) = take_digits(value, 4, 6) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix('-') else {
        return false;
    };
    let Some((month, rest)) = take_digits(rest, 2, 2) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix('-') else {
        return false;
    };
    let Some((day, rest)) = take_digits(rest, 2, 2) else {
        return false;
    };
    rest.is_empty()
        && year > 0
        && (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
}

/// The number of ISO-8601 weeks in `year` (52 or 53).
fn weeks_in_year(year: u32) -> u32 {
    // A year has 53 weeks when it starts on a Thursday, or on a Wednesday in
    // a leap year. Zeller's congruence gives the day of week for Jan 1.
    let day_of_week = {
        let y = i64::from(year) - 1;
        (y + y / 4 - y / 100 + y / 400 + 1).rem_euclid(7)
    };
    // 0 = Sunday ... 4 = Thursday, 3 = Wednesday.
    if day_of_week == 4 || (day_of_week == 3 && is_leap_year(year)) {
        53
    } else {
        52
    }
}

/// Whether `value` is a
/// [valid week string](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-week-string)
/// (`YYYY-Www`) with a week that exists in its year.
#[must_use]
pub fn is_valid_week(value: &str) -> bool {
    let Some((year, rest)) = take_digits(value, 4, 6) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix("-W") else {
        return false;
    };
    let Some((week, rest)) = take_digits(rest, 2, 2) else {
        return false;
    };
    rest.is_empty() && year > 0 && week >= 1 && week <= weeks_in_year(year)
}

/// Whether `value` is a
/// [valid time string](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-time-string)
/// (`HH:MM`, `HH:MM:SS`, or `HH:MM:SS.sss`).
#[must_use]
pub fn is_valid_time(value: &str) -> bool {
    let Some((hour, rest)) = take_digits(value, 2, 2) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(':') else {
        return false;
    };
    let Some((minute, rest)) = take_digits(rest, 2, 2) else {
        return false;
    };
    if hour > 23 || minute > 59 {
        return false;
    }
    let Some(rest) = rest.strip_prefix(':') else {
        return rest.is_empty();
    };
    let Some((second, rest)) = take_digits(rest, 2, 2) else {
        return false;
    };
    if second > 59 {
        return false;
    }
    let Some(rest) = rest.strip_prefix('.') else {
        return rest.is_empty();
    };
    let fraction = rest.bytes().take_while(u8::is_ascii_digit).count();
    (1..=3).contains(&fraction) && fraction == rest.len()
}

/// Whether `value` is a
/// [valid local date and time string](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-local-date-and-time-string):
/// a valid date, a `T` or space separator, and a valid time.
#[must_use]
pub fn is_valid_local_date_time(value: &str) -> bool {
    let Some((date, time)) = value.split_once(['T', ' ']) else {
        return false;
    };
    is_valid_date(date) && is_valid_time(time)
}

/// Whether `value` is a
/// [valid simple color](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-simple-color):
/// `#` and exactly six ASCII hex digits.
#[must_use]
pub fn is_valid_simple_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
