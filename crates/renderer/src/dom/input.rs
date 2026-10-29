//! The `input` element's type state, value modes, and value sanitization.
//!
//! The current value is the element's value slot, kept sanitized. A clear
//! dirty value flag means the `value` attribute still drives the slot, so an
//! attribute change reloads it; a user-set value sets the flag and survives
//! attribute changes
//! (<https://html.spec.whatwg.org/multipage/input.html#concept-fe-value>).
//! The IDL getter, form submission, and type changes all read this value.

use blitz_dom::{BaseDocument, Dirtiness};
use style::color::{AbsoluteColor, ColorSpace};
use style::device::Device;

use crate::Parsed;
use crate::dom::microsyntaxes::{
    format_number, valid_date, valid_float, valid_local_datetime, valid_month, valid_time,
    valid_week,
};
use crate::dom::parse::parse_double;
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
    /// use the Text state. The keyword match is ASCII case-insensitive and
    /// does not trim, because `type` is an enumerated attribute
    /// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#enumerated-attribute>).
    pub(crate) fn from_attribute(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::Text;
        };
        if value.eq_ignore_ascii_case("hidden") {
            Self::Hidden
        } else if value.eq_ignore_ascii_case("search") {
            Self::Search
        } else if value.eq_ignore_ascii_case("tel") {
            Self::Tel
        } else if value.eq_ignore_ascii_case("url") {
            Self::Url
        } else if value.eq_ignore_ascii_case("email") {
            Self::Email
        } else if value.eq_ignore_ascii_case("password") {
            Self::Password
        } else if value.eq_ignore_ascii_case("date") {
            Self::Date
        } else if value.eq_ignore_ascii_case("month") {
            Self::Month
        } else if value.eq_ignore_ascii_case("week") {
            Self::Week
        } else if value.eq_ignore_ascii_case("time") {
            Self::Time
        } else if value.eq_ignore_ascii_case("datetime-local") {
            Self::DatetimeLocal
        } else if value.eq_ignore_ascii_case("number") {
            Self::Number
        } else if value.eq_ignore_ascii_case("range") {
            Self::Range
        } else if value.eq_ignore_ascii_case("color") {
            Self::Color
        } else if value.eq_ignore_ascii_case("checkbox") {
            Self::Checkbox
        } else if value.eq_ignore_ascii_case("radio") {
            Self::Radio
        } else if value.eq_ignore_ascii_case("file") {
            Self::File
        } else if value.eq_ignore_ascii_case("submit") {
            Self::Submit
        } else if value.eq_ignore_ascii_case("image") {
            Self::Image
        } else if value.eq_ignore_ascii_case("reset") {
            Self::Reset
        } else if value.eq_ignore_ascii_case("button") {
            Self::Button
        } else {
            Self::Text
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
            Self::Hidden | Self::Submit | Self::Image | Self::Reset | Self::Button => {
                ValueMode::Default
            }
            Self::File => ValueMode::Filename,
            Self::Text
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

/// The current value the type-change steps start from
/// (<https://html.spec.whatwg.org/multipage/input.html#concept-fe-value>).
pub(crate) fn raw_value(parsed: &Parsed, node: BlitzId, kind: InputType) -> String {
    let base = &parsed.document.base;
    let Some(element) = base
        .get_node(node)
        .and_then(|target| target.data.downcast_element())
    else {
        return String::new();
    };
    // A color's synthesized default (`#000000`) is not the input's value when
    // the value attribute is absent. A type change must reload the empty
    // attribute, not carry the default into a text state
    // (<https://html.spec.whatwg.org/multipage/input.html#color-state-(type=color)>).
    if kind == InputType::Color && !element.form_state.dirty_value() {
        return attr(base, node, "value").unwrap_or("").to_owned();
    }
    element.form_state.value().to_owned()
}

/// The current value of an input, dispatched by the value IDL attribute's
/// mode (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
pub(crate) fn current_value(parsed: &Parsed, node: BlitzId) -> String {
    let base = &parsed.document.base;
    let Some(kind) = input_type(base, node) else {
        return String::new();
    };
    match kind.value_mode() {
        // The slot is sanitized whenever it is written: on parse, on the
        // attributes sanitization reads, and on every IDL or user set
        // (<https://html.spec.whatwg.org/multipage/input.html#concept-fe-value>).
        ValueMode::Current => base
            .get_node(node)
            .and_then(|target| target.data.downcast_element())
            .map_or_else(String::new, |element| element.form_state.value().to_owned()),
        ValueMode::Default => attr(base, node, "value").unwrap_or("").to_owned(),
        ValueMode::DefaultOn => attr(base, node, "value").unwrap_or("on").to_owned(),
        ValueMode::Filename => String::new(),
    }
}

/// Applies a state's value sanitization algorithm
/// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element>).
/// Applies the state's algorithm. The document's device resolves a system
/// color in the color state with the used color scheme.
pub(crate) fn sanitize(
    base: &BaseDocument,
    device: &Device,
    node: BlitzId,
    kind: InputType,
    raw: &str,
) -> String {
    match kind {
        InputType::Text | InputType::Search | InputType::Tel | InputType::Password => {
            strip_newlines(raw)
        }
        InputType::Url => strip_newlines(raw).trim_ascii().to_owned(),
        InputType::Email => {
            // The single-value algorithm strips newlines before trimming. The
            // multiple-value algorithm only splits and trims, so an interior
            // newline survives and makes the token invalid. Chrome strips the
            // newline for both; the spec's multiple-value algorithm has no
            // such step, so this follows the spec.
            // (<https://html.spec.whatwg.org/multipage/input.html#email-state-(type=email)>)
            let value = if attr(base, node, "multiple").is_some() {
                raw.split(',')
                    .map(str::trim_ascii)
                    .collect::<Vec<_>>()
                    .join(",")
            } else {
                strip_newlines(raw).trim_ascii().to_owned()
            };
            email_domains_to_ascii(&value)
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
        InputType::Time => valid_time(raw).map(|_| raw.to_owned()).unwrap_or_default(),
        InputType::DatetimeLocal => valid_local_datetime(raw)
            .map(|(date, time)| {
                format!(
                    "{date}T{}",
                    crate::dom::microsyntaxes::normalized_time(time)
                )
            })
            .unwrap_or_default(),
        InputType::Number => valid_float(raw).map(|_| raw.to_owned()).unwrap_or_default(),
        InputType::Range => sanitize_range(base, node, raw),
        InputType::Color => sanitize_color(base, device, node, raw),
        InputType::Hidden
        | InputType::Checkbox
        | InputType::Radio
        | InputType::File
        | InputType::Submit
        | InputType::Image
        | InputType::Reset
        | InputType::Button => raw.to_owned(),
    }
}

/// Converts each address's domain to its ASCII (IDNA) form, leaving addresses
/// that do not parse as a host unchanged
/// (<https://html.spec.whatwg.org/multipage/input.html#e-mail-state-(type=email)>).
fn email_domains_to_ascii(value: &str) -> String {
    value
        .split(',')
        .map(|address| {
            let Some((local, domain)) = address.rsplit_once('@') else {
                return address.to_owned();
            };
            match url::Host::parse(domain) {
                Ok(host) => format!("{local}@{host}"),
                Err(_) => address.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn strip_newlines(value: &str) -> String {
    value
        .chars()
        .filter(|character| *character != '\n' && *character != '\r')
        .collect()
}

/// The range state's value: default value when invalid, then the nearest
/// allowed step that is inside the accepted interval. The arithmetic is over
/// exact decimals; the floating-point path only runs when a value does not fit
/// in i128
/// (<https://html.spec.whatwg.org/multipage/input.html#range-state-(type=range)>).
fn sanitize_range(base: &BaseDocument, node: BlitzId, raw: &str) -> String {
    let minimum = attr(base, node, "min")
        .and_then(crate::dom::microsyntaxes::parse_decimal)
        .unwrap_or_else(crate::dom::microsyntaxes::Decimal::zero);
    let maximum = attr(base, node, "max")
        .and_then(crate::dom::microsyntaxes::parse_decimal)
        .unwrap_or_else(|| crate::dom::microsyntaxes::Decimal::from_integer(100));
    let default = crate::dom::microsyntaxes::range_default(&minimum, &maximum);
    let value = if valid_float(raw).is_some() {
        crate::dom::microsyntaxes::parse_decimal(raw).unwrap_or_else(|| default.clone())
    } else {
        default
    };
    if attr(base, node, "step").is_some_and(|step| step.eq_ignore_ascii_case("any")) {
        let clamped = if value
            .compare(&minimum)
            .is_some_and(std::cmp::Ordering::is_lt)
        {
            minimum
        } else if value
            .compare(&maximum)
            .is_some_and(std::cmp::Ordering::is_gt)
        {
            maximum
        } else {
            value
        };
        return format_number(clamped.to_f64());
    }
    let step = attr(base, node, "step")
        .and_then(crate::dom::microsyntaxes::parse_decimal)
        .filter(|step| !step.is_negative() && !step.is_zero())
        .unwrap_or_else(|| crate::dom::microsyntaxes::Decimal::from_integer(1));
    let step_base = attr(base, node, "min")
        .and_then(crate::dom::microsyntaxes::parse_decimal)
        .or_else(|| attr(base, node, "value").and_then(crate::dom::microsyntaxes::parse_decimal))
        .unwrap_or_else(crate::dom::microsyntaxes::Decimal::zero);
    match crate::dom::microsyntaxes::align_range(&minimum, &maximum, &step, &step_base, &value) {
        Some(aligned) => format_number(aligned.to_f64()),
        None => sanitize_range_f64(base, node, value.to_f64()),
    }
}

/// The floating-point fallback for range values that do not fit i128.
fn sanitize_range_f64(base: &BaseDocument, node: BlitzId, value: f64) -> String {
    let minimum = attr(base, node, "min")
        .and_then(parse_double)
        .unwrap_or(0.0);
    let maximum = attr(base, node, "max")
        .and_then(parse_double)
        .unwrap_or(100.0);
    if maximum < minimum {
        return format_number(minimum);
    }
    let step = attr(base, node, "step")
        .and_then(parse_double)
        .filter(|step| *step > 0.0)
        .unwrap_or(1.0);
    let step_base = step_base_from_attributes(base, node);
    let mut value = value.clamp(minimum, maximum);
    let quotient = (value - step_base) / step;
    let mut aligned = step_base + (quotient + 0.5).floor() * step;
    if aligned > maximum {
        let steps = ((aligned - maximum) / step).ceil();
        aligned -= steps * step;
    } else if aligned < minimum {
        let steps = ((minimum - aligned) / step).ceil();
        aligned += steps * step;
    }
    if (minimum..=maximum).contains(&aligned) {
        value = aligned;
    }
    format_number(value)
}

fn step_base_from_attributes(base: &BaseDocument, node: BlitzId) -> f64 {
    attr(base, node, "min")
        .and_then(parse_double)
        .or_else(|| attr(base, node, "value").and_then(parse_double))
        .unwrap_or(0.0)
}

/// Applies the state's algorithm after flushing pending device changes, for
/// callers that mutate the document (attribute changes, IDL setters).
pub(crate) fn sanitize_with_device(
    base: &mut BaseDocument,
    node: BlitzId,
    kind: InputType,
    raw: &str,
) -> String {
    let _device = base.stylist_device();
    let device = base.device();
    sanitize(base, device, node, kind, raw)
}

/// How the color state serializes its value: the `colorspace` attribute
/// selects Display P3, and `alpha` keeps the authored alpha
/// (<https://html.spec.whatwg.org/multipage/input.html#update-a-color-well-control-color>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ColorWellOptions {
    display_p3: bool,
    preserve_alpha: bool,
}

impl ColorWellOptions {
    fn from_attributes(base: &BaseDocument, node: BlitzId) -> Self {
        Self {
            display_p3: attr(base, node, "colorspace")
                .is_some_and(|space| space.eq_ignore_ascii_case("display-p3")),
            preserve_alpha: attr(base, node, "alpha").is_some(),
        }
    }

    fn default_value(self) -> String {
        if self.display_p3 {
            "color(display-p3 0 0 0)".to_owned()
        } else if self.preserve_alpha {
            "color(srgb 0 0 0)".to_owned()
        } else {
            "#000000".to_owned()
        }
    }
}

/// The color state's value: parse, then force alpha opaque unless the `alpha`
/// attribute is present, and serialize in sRGB or `display-p3` according to
/// the `colorspace` attribute
/// (<https://html.spec.whatwg.org/multipage/input.html#update-a-color-well-control-color>).
fn sanitize_color(base: &BaseDocument, device: &Device, node: BlitzId, raw: &str) -> String {
    let options = ColorWellOptions::from_attributes(base, node);
    let Some(parsed) = crate::css::color::parse_exact(raw, Some(device), &AbsoluteColor::BLACK)
    else {
        return options.default_value();
    };
    if options.display_p3 {
        // Keep the authored Display P3 components when the input already names
        // that space; a round trip through sRGB would perturb them.
        let p3 = parsed.absolute.to_color_space(ColorSpace::DisplayP3);
        let alpha = if options.preserve_alpha {
            p3.alpha
        } else {
            1.0
        };
        return serialize_color_function(
            "display-p3",
            [p3.components.0, p3.components.1, p3.components.2],
            alpha,
        );
    }
    if options.preserve_alpha {
        serialize_color_function(
            "srgb",
            [parsed.value[0], parsed.value[1], parsed.value[2]],
            parsed.value[3],
        )
    } else {
        let [red, green, blue, _] = parsed.bytes;
        format!("#{red:02x}{green:02x}{blue:02x}")
    }
}

fn serialize_color_function(space: &str, [red, green, blue]: [f32; 3], alpha: f32) -> String {
    // Six decimal places match the precision the color state exposes; it also
    // removes the f32 noise from the sRGB <-> Display P3 conversion.
    let component = |value: f32| {
        let value = (f64::from(value) * 1_000_000.0).round() / 1_000_000.0;
        format_number(value)
    };
    let red = component(red);
    let green = component(green);
    let blue = component(blue);
    if alpha >= 1.0 {
        format!("color({space} {red} {green} {blue})")
    } else {
        format!(
            "color({space} {red} {green} {blue} / {})",
            component(alpha.clamp(0.0, 1.0))
        )
    }
}

/// Recomputes an input's sanitized value from its content attributes and
/// writes it into the value slot and Blitz's layout editor. A clear dirty
/// value flag means the `value` attribute drives the slot; a set one keeps
/// the current value, except for the states whose sanitization also reads
/// `min`, `max`, `step`, `colorspace`, `alpha`, or `multiple`
/// (<https://html.spec.whatwg.org/multipage/input.html#concept-fe-value>).
pub(crate) fn sync_sanitized_value(document: &mut crate::documents::BlitzDocument, node: BlitzId) {
    let base = &document.base;
    let Some(kind) = input_type(base, node) else {
        return;
    };
    if kind.value_mode() != ValueMode::Current {
        return;
    }
    let Some(element) = base
        .get_node(node)
        .and_then(|target| target.data.downcast_element())
    else {
        return;
    };
    let dirty = element.form_state.dirty_value();
    if dirty && !matches!(kind, InputType::Range | InputType::Color | InputType::Email) {
        return;
    }
    let raw = if dirty {
        element.form_state.value().to_owned()
    } else {
        attr(base, node, "value").unwrap_or("").to_owned()
    };
    // The slot this writes feeds every later read, so a system color has to
    // resolve with the document's device here.
    let sanitized = sanitize_with_device(&mut document.base, node, kind, &raw);
    document
        .base
        .mutate()
        .set_value(node, sanitized, Dirtiness::Keep);
}
