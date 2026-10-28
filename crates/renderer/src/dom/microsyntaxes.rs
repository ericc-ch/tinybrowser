//! HTML common microsyntaxes shared by form controls and script plumbing.
//!
//! The algorithms follow the spec section on common microsyntaxes
//! (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html>), with
//! the number serializer following ECMAScript `Number::toString`
//! (<https://tc39.es/ecma262/#sec-numeric-types-number-tostring>).

/// Parses a whole string as a valid floating-point number
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-floating-point-number>).
pub(crate) fn valid_float(value: &str) -> Option<f64> {
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
            return None;
        }
    }
    if integer_digits == 0 && fraction_digits == 0 {
        return None;
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
            return None;
        }
    }
    if position != bytes.len() {
        return None;
    }
    value
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
}

/// The best representation of a number as a floating-point number, i.e.
/// ECMAScript `Number::toString` for radix 10
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#best-representation-of-the-number-as-a-floating-point-number>).
pub(crate) fn format_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "Infinity".to_owned()
        } else {
            "-Infinity".to_owned()
        };
    }
    let negative = value.is_sign_negative();
    let magnitude = value.abs();
    let scientific = format!("{magnitude:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent = exponent.parse::<i32>().unwrap_or(0);
    let digits: String = mantissa
        .chars()
        .filter(|character| *character != '.')
        .collect();
    let k = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!(
            "{digits}{}",
            "0".repeat(usize::try_from(n - k).unwrap_or(0))
        )
    } else if 0 < n && n <= 21 {
        let split = usize::try_from(n).unwrap_or(digits.len());
        format!("{}.{}", &digits[..split], &digits[split..])
    } else if -6 < n && n <= 0 {
        format!(
            "0.{}{}",
            "0".repeat(usize::try_from(-n).unwrap_or(0)),
            digits
        )
    } else {
        let exponent = n - 1;
        let sign = if exponent >= 0 { '+' } else { '-' };
        let mantissa = if k == 1 {
            digits
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        format!("{mantissa}e{sign}{}", exponent.abs())
    };
    if negative { format!("-{body}") } else { body }
}

fn is_leap_year(year: u64) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn parse_year(value: &str) -> Option<u64> {
    if value.len() < 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().filter(|year| *year > 0)
}

fn parse_two_digits(value: &str) -> Option<u64> {
    if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok()
}

/// A valid date string: year > 0, a real month, and a day in that month
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#valid-date-string>).
pub(crate) fn valid_date(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(year) = parts.next().and_then(parse_year) else {
        return false;
    };
    let Some(month) = parts.next().and_then(parse_two_digits) else {
        return false;
    };
    let Some(day) = parts.next().and_then(parse_two_digits) else {
        return false;
    };
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return false;
    }
    (1..=days_in_month(year, month)).contains(&day)
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

/// The day of the week for January 1, with Monday = 0. Year 1 started on a
/// Monday in the proleptic Gregorian calendar.
fn january_first_weekday(year: u64) -> u64 {
    let previous = u128::from(year - 1);
    let days = 365 * previous + previous / 4 - previous / 100 + previous / 400;
    u64::try_from(days % 7).unwrap_or(0)
}

/// A valid week string: year > 0, uppercase `W`, and a week present in that
/// week-year. A year has 53 weeks exactly when January 1 is a Thursday or a
/// leap-year Wednesday
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
    let weeks = if january_first_weekday(year) == 3
        || (is_leap_year(year) && january_first_weekday(year) == 2)
    {
        53
    } else {
        52
    };
    (1..=weeks).contains(&week)
}

/// A parsed time: hour, minute, second, and milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Time {
    pub(crate) hour: u64,
    pub(crate) minute: u64,
    pub(crate) second: u64,
    pub(crate) millis: u64,
}

/// A valid time string: `hh:mm`, optional `:ss`, and a fraction only after
/// the seconds component
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
        Some(second) => Some(parse_two_digits(second)?),
        None => None,
    };
    if parts.next().is_some()
        || hour > 23
        || minute > 59
        || second.is_some_and(|second| second > 59)
    {
        return None;
    }
    if fraction.is_some() && second.is_none() {
        return None;
    }
    let second = second.unwrap_or(0);
    let millis = match fraction {
        Some(fraction) => {
            if fraction.is_empty()
                || fraction.len() > 3
                || !fraction.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            let mut padded = fraction.to_owned();
            while padded.len() < 3 {
                padded.push('0');
            }
            padded.parse::<u64>().ok()?
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
    let seconds = if time.second != 0 || time.millis != 0 {
        format!(":{:02}", time.second)
    } else {
        String::new()
    };
    let fraction = if time.millis != 0 {
        let fraction = format!("{:03}", time.millis);
        format!(".{}", fraction.trim_end_matches('0'))
    } else {
        String::new()
    };
    format!("{:02}:{:02}{seconds}{fraction}", time.hour, time.minute)
}

/// An exact decimal parsed from a floating-point attribute. Step alignment is
/// defined over decimal numbers, not binary floats, so `0.1 + 0.2` must not
/// produce `0.30000000000000004`
/// (<https://html.spec.whatwg.org/multipage/input.html#attr-input-step>).
#[derive(Clone, Debug)]
pub(crate) struct Decimal {
    negative: bool,
    digits: String,
    scale: u32,
}

impl Decimal {
    pub(crate) fn zero() -> Self {
        Self {
            negative: false,
            digits: "0".to_owned(),
            scale: 0,
        }
    }

    pub(crate) fn from_integer(value: i128) -> Self {
        let negative = value < 0;
        Self {
            negative,
            digits: value.unsigned_abs().to_string(),
            scale: 0,
        }
    }

    pub(crate) fn is_negative(&self) -> bool {
        self.negative && self.digits != "0"
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.digits == "0"
    }

    /// The signed mantissa at `scale`, or `None` when it does not fit i128.
    fn mantissa_at(&self, scale: u32) -> Option<i128> {
        let zeros = scale.checked_sub(self.scale)?;
        let zeros = usize::try_from(zeros).ok()?;
        let mut text = String::with_capacity(self.digits.len() + zeros);
        text.push_str(&self.digits);
        text.extend(std::iter::repeat_n('0', zeros));
        let magnitude = text.parse::<i128>().ok()?;
        Some(if self.negative { -magnitude } else { magnitude })
    }

    fn from_mantissa(mantissa: i128, scale: u32) -> Self {
        let negative = mantissa < 0;
        let mut decimal = Self {
            negative,
            digits: mantissa.unsigned_abs().to_string(),
            scale,
        };
        decimal.normalize();
        decimal
    }

    fn normalize(&mut self) {
        while self.digits.len() > 1 && self.digits.starts_with('0') {
            self.digits.remove(0);
        }
        while self.scale > 0 && self.digits.ends_with('0') {
            self.digits.pop();
            self.scale -= 1;
        }
        if self.digits == "0" {
            self.negative = false;
            self.scale = 0;
        }
    }

    pub(crate) fn to_f64(&self) -> f64 {
        let sign = if self.negative { "-" } else { "" };
        format!("{sign}{}e-{}", self.digits, self.scale)
            .parse()
            .unwrap_or(0.0)
    }

    pub(crate) fn compare(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let scale = self.scale.max(other.scale);
        Some(self.mantissa_at(scale)?.cmp(&other.mantissa_at(scale)?))
    }
}

/// Parses the longest decimal prefix with the rules for parsing
/// floating-point number values. The caller decides whether trailing junk is
/// allowed (attributes) or the whole string must be a valid number (values).
pub(crate) fn parse_decimal(input: &str) -> Option<Decimal> {
    let bytes = input.as_bytes();
    let mut position = 0;
    while bytes.get(position).is_some_and(u8::is_ascii_whitespace) {
        position += 1;
    }
    let negative = match bytes.get(position) {
        Some(b'-') => {
            position += 1;
            true
        }
        Some(b'+') => {
            position += 1;
            false
        }
        _ => false,
    };
    let mut digits = String::new();
    let mut fraction_digits = 0_u32;
    while bytes.get(position).is_some_and(u8::is_ascii_digit) {
        digits.push(char::from(bytes[position]));
        position += 1;
    }
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            digits.push(char::from(bytes[position]));
            fraction_digits += 1;
            position += 1;
        }
    }
    if digits.is_empty() {
        return None;
    }
    let mut exponent = 0_i64;
    if bytes
        .get(position)
        .is_some_and(|byte| *byte == b'e' || *byte == b'E')
    {
        let mut cursor = position + 1;
        let exponent_negative = match bytes.get(cursor) {
            Some(b'-') => {
                cursor += 1;
                true
            }
            Some(b'+') => {
                cursor += 1;
                false
            }
            _ => false,
        };
        let start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            exponent = exponent
                .saturating_mul(10)
                .saturating_add(i64::from(bytes[cursor] - b'0'));
            cursor += 1;
        }
        if cursor > start && exponent_negative {
            exponent = -exponent;
        }
    }
    // A value that rounds to infinity is a parse error, and one that rounds
    // to zero is zero. Bound the exponent before expanding it, so an
    // attribute such as `min="1e99999999999999999999"` cannot allocate an
    // enormous digit string or abort the renderer
    // (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-floating-point-number-values>).
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some(Decimal::zero());
    }
    let significant_len = i64::try_from(significant.len()).unwrap_or(i64::MAX);
    let magnitude = significant_len
        .saturating_sub(1)
        .saturating_add(exponent)
        .saturating_sub(i64::from(fraction_digits));
    if magnitude > 309 {
        return None;
    }
    if magnitude < -324 {
        return Some(Decimal::zero());
    }
    let mut scale = i64::from(fraction_digits) - exponent;
    if scale < 0 {
        let zeros = usize::try_from(-scale).ok()?;
        digits.extend(std::iter::repeat_n('0', zeros));
        scale = 0;
    }
    let mut decimal = Decimal {
        negative,
        digits,
        scale: u32::try_from(scale).ok()?,
    };
    decimal.normalize();
    // The parsing rules reject a value that rounds to infinity and return zero
    // for one that rounds to zero
    // (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-floating-point-number-values>).
    let number = decimal.to_f64();
    if !number.is_finite() {
        return None;
    }
    if number == 0.0 {
        return Some(Decimal::zero());
    }
    Some(decimal)
}

/// The exact range state value: default when invalid, then the nearest step
/// on the grid that stays inside `[min, max]`
/// (<https://html.spec.whatwg.org/multipage/input.html#range-state-(type=range)>).
/// Returns `None` when the operands do not fit i128, so the caller can use the
/// floating-point fallback.
pub(crate) fn align_range(
    minimum: &Decimal,
    maximum: &Decimal,
    step: &Decimal,
    step_base: &Decimal,
    value: &Decimal,
) -> Option<Decimal> {
    if maximum.compare(minimum)? == std::cmp::Ordering::Less {
        return Some(minimum.clone());
    }
    let scale = value
        .scale
        .max(maximum.scale)
        .max(minimum.scale)
        .max(step.scale)
        .max(step_base.scale);
    let value_at = value.mantissa_at(scale)?;
    let minimum_at = minimum.mantissa_at(scale)?;
    let maximum_at = maximum.mantissa_at(scale)?;
    let step_base_at = step_base.mantissa_at(scale)?;
    let step_at = step.mantissa_at(scale)?;
    if step_at <= 0 {
        return None;
    }
    // floor((n / d) + 0.5): ties round toward positive infinity.
    let numerator = value_at
        .checked_sub(step_base_at)?
        .checked_mul(2)?
        .checked_add(step_at)?;
    let denominator = step_at.checked_mul(2)?;
    let quotient = numerator.div_euclid(denominator);
    let mut aligned = step_base_at.checked_add(quotient.checked_mul(step_at)?)?;
    if aligned > maximum_at {
        let steps = aligned
            .checked_sub(maximum_at)?
            .checked_add(step_at)?
            .checked_sub(1)?
            .div_euclid(step_at);
        aligned = aligned.checked_sub(steps.checked_mul(step_at)?)?;
    } else if aligned < minimum_at {
        let steps = minimum_at
            .checked_sub(aligned)?
            .checked_add(step_at)?
            .checked_sub(1)?
            .div_euclid(step_at);
        aligned = aligned.checked_add(steps.checked_mul(step_at)?)?;
    }
    if aligned < minimum_at || aligned > maximum_at {
        // No grid point is inside the interval. The value still clamps to the
        // interval, as the overflow and underflow steps run before stepping
        // (<https://html.spec.whatwg.org/multipage/input.html#range-state-(type=range)>).
        return Some(Decimal::from_mantissa(
            value_at.clamp(minimum_at, maximum_at),
            scale,
        ));
    }
    Some(Decimal::from_mantissa(aligned, scale))
}

/// The range state's default value: the midpoint of `[min, max]`, or `min`
/// when the interval is inverted.
pub(crate) fn range_default(minimum: &Decimal, maximum: &Decimal) -> Decimal {
    if maximum
        .compare(minimum)
        .is_none_or(std::cmp::Ordering::is_lt)
    {
        return minimum.clone();
    }
    let scale = minimum.scale.max(maximum.scale);
    let Some(minimum_at) = minimum.mantissa_at(scale) else {
        return minimum.clone();
    };
    let Some(maximum_at) = maximum.mantissa_at(scale) else {
        return minimum.clone();
    };
    let Some(difference) = maximum_at.checked_sub(minimum_at) else {
        return minimum.clone();
    };
    // (min + (max - min) / 2) at one extra decimal place.
    let Some(half_scaled) = minimum_at.checked_mul(10).and_then(|minimum| {
        difference
            .checked_mul(5)
            .and_then(|half| minimum.checked_add(half))
    }) else {
        return minimum.clone();
    };
    let Some(scale) = scale.checked_add(1) else {
        return minimum.clone();
    };
    Decimal::from_mantissa(half_scaled, scale)
}

/// `crossorigin`
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#attr-crossorigin>).
///
/// An absent attribute is `None`, which means no CORS request at all. An empty
/// or unknown value is `Anonymous`, the spec's "invalid value default", so an
/// invalid spelling never sends credentials
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#enumerated-attribute>).
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CrossOrigin {
    Anonymous,
    UseCredentials,
}

impl CrossOrigin {
    /// Every keyword, in spec order.
    pub const KEYWORDS: &'static [Self] = &[Self::Anonymous, Self::UseCredentials];

    /// The canonical spelling of the keyword.
    #[must_use]
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::UseCredentials => "use-credentials",
        }
    }

    /// Parses one keyword, ASCII case-insensitively and untrimmed, the way an
    /// enumerated attribute does.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::KEYWORDS
            .iter()
            .copied()
            .find(|candidate| candidate.keyword().eq_ignore_ascii_case(value))
    }

    /// The attribute value as an enum: absent stays absent, and an empty or
    /// unknown value is `Anonymous`.
    #[must_use]
    pub fn from_attr(value: Option<&str>) -> Option<Self> {
        value.map(|value| Self::parse(value).unwrap_or(Self::Anonymous))
    }

    /// The credentials mode this attribute selects for a CORS request
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
    #[must_use]
    pub fn credentials(self) -> net::cors::CredentialsMode {
        match self {
            Self::Anonymous => net::cors::CredentialsMode::SameOrigin,
            Self::UseCredentials => net::cors::CredentialsMode::Include,
        }
    }
}

#[cfg(test)]
mod cross_origin_tests {
    use super::CrossOrigin;
    use net::cors::CredentialsMode;

    #[test]
    fn an_invalid_or_empty_crossorigin_never_sends_credentials() {
        assert_eq!(CrossOrigin::from_attr(None), None);
        assert_eq!(
            CrossOrigin::from_attr(Some("")),
            Some(CrossOrigin::Anonymous)
        );
        // Enumerated attributes are an untrimmed match, so a stray space is an
        // invalid value, and the invalid-value default is anonymous. The
        // reflected `img.crossOrigin` and the load must agree.
        assert_eq!(
            CrossOrigin::from_attr(Some(" use-credentials")),
            Some(CrossOrigin::Anonymous)
        );
        assert_eq!(
            CrossOrigin::from_attr(Some("use-credentials")),
            Some(CrossOrigin::UseCredentials)
        );
        assert_eq!(
            CrossOrigin::from_attr(Some("USE-CREDENTIALS")),
            Some(CrossOrigin::UseCredentials)
        );
        assert_eq!(
            CrossOrigin::UseCredentials.credentials(),
            CredentialsMode::Include
        );
        assert_eq!(
            CrossOrigin::Anonymous.credentials(),
            CredentialsMode::SameOrigin
        );
    }
}
