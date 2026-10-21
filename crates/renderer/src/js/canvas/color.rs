//! CSS `<color>` parsing for canvas styles.
//!
//! Canvas accepts any CSS color where it takes a color
//! (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-fillstyle>),
//! so the parser follows CSS Color 4 for the sRGB syntaxes. Wide-gamut
//! functions (`lab()`, `oklch()`, `color()`) are not parsed yet and are
//! treated as invalid assignments, leaving the previous value in place.

/// A parsed color plus the exact CSS serialization, when the syntax has no
/// sRGB-byte equivalent (`color()`, relative colors, `color-mix()`).
pub(crate) struct ParsedColor {
    pub(crate) bytes: [u8; 4],
    pub(crate) value: [f32; 4],
    pub(crate) serialized: Option<String>,
}

/// Parses the color syntaxes that canvas may need to report as `color(srgb …)`
/// in addition to the legacy sRGB forms.
pub(crate) fn parse_exact(input: &str) -> Option<ParsedColor> {
    let input = input.trim().to_ascii_lowercase();
    if let Some(inner) = function_args(&input, "color-mix") {
        return parse_color_mix(inner);
    }
    if let Some(inner) = function_args(&input, "rgb")
        && let Some(relative) = inner.trim().strip_prefix("from ")
    {
        return parse_relative(relative, false);
    }
    if let Some(inner) = function_args(&input, "color") {
        if let Some(relative) = inner.trim().strip_prefix("from ") {
            return parse_relative(relative, true);
        }
        return parse_color_function(inner);
    }
    if let Some(inner) = function_args(&input, "lab") {
        return parse_lab(inner);
    }
    parse(&input).map(|bytes| ParsedColor {
        bytes,
        value: from_bytes(bytes),
        serialized: None,
    })
}

/// Parse a CSS color into straight (non-premultiplied) sRGB bytes.
pub(crate) fn parse(input: &str) -> Option<[u8; 4]> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let lower = input.to_ascii_lowercase();
    if lower == "transparent" {
        return Some([0, 0, 0, 0]);
    }
    if let Some(hex) = lower.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(args) = function_args(&lower, "rgb") {
        return parse_rgb(&split_args(args));
    }
    if let Some(args) = function_args(&lower, "rgba") {
        return parse_rgb(&split_args(args));
    }
    if let Some(args) = function_args(&lower, "hsl") {
        return parse_hsl(&split_args(args));
    }
    if let Some(args) = function_args(&lower, "hsla") {
        return parse_hsl(&split_args(args));
    }
    if let Some(args) = function_args(&lower, "hwb") {
        return parse_hwb(&split_args(args));
    }
    named(&lower)
}

fn parse_hex(hex: &str) -> Option<[u8; 4]> {
    let digits: Vec<u8> = hex
        .bytes()
        .map(|byte| {
            (byte as char)
                .to_digit(16)
                .and_then(|value| u8::try_from(value).ok())
        })
        .collect::<Option<_>>()?;
    let short = |value: u8| value * 17;
    match digits.len() {
        3 => Some([short(digits[0]), short(digits[1]), short(digits[2]), 255]),
        4 => Some([
            short(digits[0]),
            short(digits[1]),
            short(digits[2]),
            short(digits[3]),
        ]),
        6 => Some([
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
            255,
        ]),
        8 => Some([
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
            digits[6] * 16 + digits[7],
        ]),
        _ => None,
    }
}

/// Splits `name(...)` into its argument string, lowercased. A color function
/// at EOF is closed by the parser, so the final `)` may be missing
/// (<https://drafts.csswg.org/css-syntax-3/#consume-function>).
fn function_args<'a>(input: &'a str, name: &str) -> Option<&'a str> {
    let rest = input.strip_prefix(name)?.strip_prefix('(')?;
    Some(rest.strip_suffix(')').unwrap_or(rest))
}

/// One color function's components, separated by commas or whitespace, with
/// the alpha after `/`.
struct Args {
    components: Vec<String>,
    alpha: Option<String>,
    /// Legacy comma-separated syntax, which cannot carry a `/` alpha.
    comma: bool,
}

fn split_args(input: &str) -> Args {
    let (body, alpha) = match input.split_once('/') {
        Some((body, alpha)) => (body, Some(alpha.trim().to_owned())),
        None => (input, None),
    };
    let comma = body.contains(',');
    let components = if comma {
        body.split(',').map(|part| part.trim().to_owned()).collect()
    } else {
        body.split_whitespace().map(ToOwned::to_owned).collect()
    };
    Args {
        components,
        alpha,
        comma,
    }
}

/// A CSS number. Values beyond `f32` are saturated instead of rejected so
/// `rgb()` clamps them like any other out-of-range component
/// (<https://drafts.csswg.org/css-syntax-3/#consume-number>).
fn number(text: &str) -> Option<f32> {
    // `1.` is not a CSS number; the digit after the dot is mandatory.
    if text.ends_with('.') {
        return None;
    }
    let value = text.parse::<f64>().ok()?;
    if value.is_nan() {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is saturated to the f32 range before any channel clamp"
    )]
    let value = value.clamp(-1.0e30, 1.0e30) as f32;
    Some(value)
}

fn percentage(text: &str) -> Option<f32> {
    number(text.strip_suffix('%')?).map(|value| value / 100.0)
}

fn alpha(text: Option<&str>) -> Option<u8> {
    let Some(text) = text else { return Some(255) };
    let value = number(text).or_else(|| percentage(text))?;
    Some(channel(value))
}

/// CSS clamps an sRGB channel 0..1 to 0..255 with rounding.
fn channel(value: f32) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is clamped to 0..=255 before the cast"
    )]
    let channel = value.mul_add(255.0, 0.5).clamp(0.0, 255.0) as u8;
    channel
}

fn rgb_component(text: &str) -> Option<f32> {
    if let Some(percent) = percentage(text) {
        return Some(percent);
    }
    number(text).map(|value| value / 255.0)
}

fn legacy_alpha(args: &Args) -> Option<(&[String], Option<&str>)> {
    match (args.components.len(), args.alpha.as_deref()) {
        (3, alpha) => Some((&args.components, alpha)),
        (4, None) => Some((&args.components[..3], Some(args.components[3].as_str()))),
        _ => None,
    }
}

fn parse_rgb(args: &Args) -> Option<[u8; 4]> {
    if args.comma && args.alpha.is_some() {
        return None;
    }
    let (components, alpha_text) = legacy_alpha(args)?;
    if args.comma {
        // Legacy comma syntax cannot mix percentages and numbers.
        let percentages = components[0].ends_with('%');
        if components
            .iter()
            .any(|component| component.ends_with('%') != percentages)
        {
            return None;
        }
    }
    let mut rgb = [0u8; 3];
    for (slot, text) in rgb.iter_mut().zip(components) {
        // Modern space-separated syntax accepts the `none` keyword, which
        // behaves as zero for legacy sRGB
        // (<https://drafts.csswg.org/css-color-4/#none-color>).
        let component = if !args.comma && text.eq_ignore_ascii_case("none") {
            0.0
        } else {
            rgb_component(text)?
        };
        *slot = channel(component);
    }
    Some([rgb[0], rgb[1], rgb[2], alpha(alpha_text)?])
}

fn hue(text: &str) -> Option<f32> {
    let (value, factor) = if let Some(value) = text.strip_suffix("deg") {
        (number(value)?, 1.0)
    } else if let Some(value) = text.strip_suffix("grad") {
        (number(value)?, 0.9)
    } else if let Some(value) = text.strip_suffix("rad") {
        (number(value)?, 180.0 / std::f32::consts::PI)
    } else if let Some(value) = text.strip_suffix("turn") {
        (number(value)?, 360.0)
    } else {
        (number(text)?, 1.0)
    };
    Some((value * factor).rem_euclid(360.0))
}

fn parse_hsl(args: &Args) -> Option<[u8; 4]> {
    if args.comma && args.alpha.is_some() {
        return None;
    }
    let (components, alpha_text) = legacy_alpha(args)?;
    let hue = hue(&components[0])? / 360.0;
    let saturation = percentage(&components[1])?.clamp(0.0, 1.0);
    let lightness = percentage(&components[2])?.clamp(0.0, 1.0);
    let alpha = alpha(alpha_text)?;
    let (red, green, blue) = hsl_to_rgb(hue, saturation, lightness);
    Some([channel(red), channel(green), channel(blue), alpha])
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (f32, f32, f32) {
    if saturation == 0.0 {
        return (lightness, lightness, lightness);
    }
    let high = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let low = 2.0 * lightness - high;
    (
        hue_to_rgb(low, high, hue + 1.0 / 3.0),
        hue_to_rgb(low, high, hue),
        hue_to_rgb(low, high, hue - 1.0 / 3.0),
    )
}

fn hue_to_rgb(low: f32, high: f32, position: f32) -> f32 {
    let position = position.rem_euclid(1.0);
    if position < 1.0 / 6.0 {
        (high - low).mul_add(6.0 * position, low)
    } else if position < 1.0 / 2.0 {
        high
    } else if position < 2.0 / 3.0 {
        (high - low).mul_add((2.0 / 3.0 - position) * 6.0, low)
    } else {
        low
    }
}

fn parse_hwb(args: &Args) -> Option<[u8; 4]> {
    if args.comma && args.alpha.is_some() {
        return None;
    }
    let (components, alpha_text) = legacy_alpha(args)?;
    let hue = hue(&components[0])? / 360.0;
    let whiteness = percentage(&components[1])?.clamp(0.0, 1.0);
    let blackness = percentage(&components[2])?.clamp(0.0, 1.0);
    let alpha = alpha(alpha_text)?;
    if whiteness + blackness >= 1.0 {
        let gray = channel(whiteness / (whiteness + blackness));
        return Some([gray, gray, gray, alpha]);
    }
    let (red, green, blue) = hsl_to_rgb(hue, 1.0, 0.5);
    let scale = 1.0 - whiteness - blackness;
    let adjust = |value: f32| channel(value.mul_add(scale, whiteness));
    Some([adjust(red), adjust(green), adjust(blue), alpha])
}

fn named(name: &str) -> Option<[u8; 4]> {
    if let Some(rgb) = NAMED.iter().find(|(key, _)| *key == name) {
        let rgb = rgb.1.to_be_bytes();
        return Some([rgb[1], rgb[2], rgb[3], 255]);
    }
    // CSS system colors resolve to the UA's palette. tinybrowser has no theme,
    // so they resolve to black; the important part is that they parse
    // (<https://drafts.csswg.org/css-color-4/#css-system-colors>).
    matches!(
        name,
        "accentcolor"
            | "accentcolortext"
            | "activetext"
            | "buttonborder"
            | "buttonface"
            | "buttontext"
            | "canvas"
            | "canvastext"
            | "field"
            | "fieldtext"
            | "graytext"
            | "highlight"
            | "highlighttext"
            | "linktext"
            | "mark"
            | "marktext"
            | "selecteditem"
            | "selecteditemtext"
            | "visitedtext"
            | "activeborder"
            | "activecaption"
            | "appworkspace"
            | "background"
            | "buttonhighlight"
            | "buttonshadow"
            | "captiontext"
            | "inactiveborder"
            | "inactivecaption"
            | "inactivecaptiontext"
            | "infobackground"
            | "infotext"
            | "menu"
            | "menutext"
            | "scrollbar"
            | "threeddarkshadow"
            | "threedface"
            | "threedhighlight"
            | "threedlightshadow"
            | "threedshadow"
            | "window"
            | "windowframe"
            | "windowtext"
    )
    .then_some([0, 0, 0, 255])
}

/// CSS named colors, `0xRRGGBB`.
static NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0x00f0_f8ff),
    ("antiquewhite", 0x00fa_ebd7),
    ("aqua", 0x0000_ffff),
    ("aquamarine", 0x007f_ffd4),
    ("azure", 0x00f0_ffff),
    ("beige", 0x00f5_f5dc),
    ("bisque", 0x00ff_e4c4),
    ("black", 0x0000_0000),
    ("blanchedalmond", 0x00ff_ebcd),
    ("blue", 0x0000_00ff),
    ("blueviolet", 0x008a_2be2),
    ("brown", 0x00a5_2a2a),
    ("burlywood", 0x00de_b887),
    ("cadetblue", 0x005f_9ea0),
    ("chartreuse", 0x007f_ff00),
    ("chocolate", 0x00d2_691e),
    ("coral", 0x00ff_7f50),
    ("cornflowerblue", 0x0064_95ed),
    ("cornsilk", 0x00ff_f8dc),
    ("crimson", 0x00dc_143c),
    ("cyan", 0x0000_ffff),
    ("darkblue", 0x0000_008b),
    ("darkcyan", 0x0000_8b8b),
    ("darkgoldenrod", 0x00b8_860b),
    ("darkgray", 0x00a9_a9a9),
    ("darkgreen", 0x0000_6400),
    ("darkgrey", 0x00a9_a9a9),
    ("darkkhaki", 0x00bd_b76b),
    ("darkmagenta", 0x008b_008b),
    ("darkolivegreen", 0x0055_6b2f),
    ("darkorange", 0x00ff_8c00),
    ("darkorchid", 0x0099_32cc),
    ("darkred", 0x008b_0000),
    ("darksalmon", 0x00e9_967a),
    ("darkseagreen", 0x008f_bc8f),
    ("darkslateblue", 0x0048_3d8b),
    ("darkslategray", 0x002f_4f4f),
    ("darkslategrey", 0x002f_4f4f),
    ("darkturquoise", 0x0000_ced1),
    ("darkviolet", 0x0094_00d3),
    ("deeppink", 0x00ff_1493),
    ("deepskyblue", 0x0000_bfff),
    ("dimgray", 0x0069_6969),
    ("dimgrey", 0x0069_6969),
    ("dodgerblue", 0x001e_90ff),
    ("firebrick", 0x00b2_2222),
    ("floralwhite", 0x00ff_faf0),
    ("forestgreen", 0x0022_8b22),
    ("fuchsia", 0x00ff_00ff),
    ("gainsboro", 0x00dc_dcdc),
    ("ghostwhite", 0x00f8_f8ff),
    ("gold", 0x00ff_d700),
    ("goldenrod", 0x00da_a520),
    ("gray", 0x0080_8080),
    ("green", 0x0000_8000),
    ("greenyellow", 0x00ad_ff2f),
    ("grey", 0x0080_8080),
    ("honeydew", 0x00f0_fff0),
    ("hotpink", 0x00ff_69b4),
    ("indianred", 0x00cd_5c5c),
    ("indigo", 0x004b_0082),
    ("ivory", 0x00ff_fff0),
    ("khaki", 0x00f0_e68c),
    ("lavender", 0x00e6_e6fa),
    ("lavenderblush", 0x00ff_f0f5),
    ("lawngreen", 0x007c_fc00),
    ("lemonchiffon", 0x00ff_facd),
    ("lightblue", 0x00ad_d8e6),
    ("lightcoral", 0x00f0_8080),
    ("lightcyan", 0x00e0_ffff),
    ("lightgoldenrodyellow", 0x00fa_fad2),
    ("lightgray", 0x00d3_d3d3),
    ("lightgreen", 0x0090_ee90),
    ("lightgrey", 0x00d3_d3d3),
    ("lightpink", 0x00ff_b6c1),
    ("lightsalmon", 0x00ff_a07a),
    ("lightseagreen", 0x0020_b2aa),
    ("lightskyblue", 0x0087_cefa),
    ("lightslategray", 0x0077_8899),
    ("lightslategrey", 0x0077_8899),
    ("lightsteelblue", 0x00b0_c4de),
    ("lightyellow", 0x00ff_ffe0),
    ("lime", 0x0000_ff00),
    ("limegreen", 0x0032_cd32),
    ("linen", 0x00fa_f0e6),
    ("magenta", 0x00ff_00ff),
    ("maroon", 0x0080_0000),
    ("mediumaquamarine", 0x0066_cdaa),
    ("mediumblue", 0x0000_00cd),
    ("mediumorchid", 0x00ba_55d3),
    ("mediumpurple", 0x0093_70db),
    ("mediumseagreen", 0x003c_b371),
    ("mediumslateblue", 0x007b_68ee),
    ("mediumspringgreen", 0x0000_fa9a),
    ("mediumturquoise", 0x0048_d1cc),
    ("mediumvioletred", 0x00c7_1585),
    ("midnightblue", 0x0019_1970),
    ("mintcream", 0x00f5_fffa),
    ("mistyrose", 0x00ff_e4e1),
    ("moccasin", 0x00ff_e4b5),
    ("navajowhite", 0x00ff_dead),
    ("navy", 0x0000_0080),
    ("oldlace", 0x00fd_f5e6),
    ("olive", 0x0080_8000),
    ("olivedrab", 0x006b_8e23),
    ("orange", 0x00ff_a500),
    ("orangered", 0x00ff_4500),
    ("orchid", 0x00da_70d6),
    ("palegoldenrod", 0x00ee_e8aa),
    ("palegreen", 0x0098_fb98),
    ("paleturquoise", 0x00af_eeee),
    ("palevioletred", 0x00db_7093),
    ("papayawhip", 0x00ff_efd5),
    ("peachpuff", 0x00ff_dab9),
    ("peru", 0x00cd_853f),
    ("pink", 0x00ff_c0cb),
    ("plum", 0x00dd_a0dd),
    ("powderblue", 0x00b0_e0e6),
    ("purple", 0x0080_0080),
    ("rebeccapurple", 0x0066_3399),
    ("red", 0x00ff_0000),
    ("rosybrown", 0x00bc_8f8f),
    ("royalblue", 0x0041_69e1),
    ("saddlebrown", 0x008b_4513),
    ("salmon", 0x00fa_8072),
    ("sandybrown", 0x00f4_a460),
    ("seagreen", 0x002e_8b57),
    ("seashell", 0x00ff_f5ee),
    ("sienna", 0x00a0_522d),
    ("silver", 0x00c0_c0c0),
    ("skyblue", 0x0087_ceeb),
    ("slateblue", 0x006a_5acd),
    ("slategray", 0x0070_8090),
    ("slategrey", 0x0070_8090),
    ("snow", 0x00ff_fafa),
    ("springgreen", 0x0000_ff7f),
    ("steelblue", 0x0046_82b4),
    ("tan", 0x00d2_b48c),
    ("teal", 0x0000_8080),
    ("thistle", 0x00d8_bfd8),
    ("tomato", 0x00ff_6347),
    ("turquoise", 0x0040_e0d0),
    ("violet", 0x00ee_82ee),
    ("wheat", 0x00f5_deb3),
    ("white", 0x00ff_ffff),
    ("whitesmoke", 0x00f5_f5f5),
    ("yellow", 0x00ff_ff00),
    ("yellowgreen", 0x009a_cd32),
];

/// Splits a relative-color body into its source color and channel list.
fn split_source(rest: &str) -> Option<(&str, &str)> {
    let mut depth = 0_u32;
    for (index, ch) in rest.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ' ' | '\t' if depth == 0 => {
                let source = &rest[..index];
                let channels = rest[index..].trim();
                if !source.is_empty() && !channels.is_empty() {
                    return Some((source, channels));
                }
            }
            _ => {}
        }
    }
    None
}

fn split_commas(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_u32;
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(text[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts
}

fn format_number(value: f32) -> String {
    let mut text = format!("{value:.4}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" {
        "0".to_owned()
    } else {
        text
    }
}

fn from_bytes(bytes: [u8; 4]) -> [f32; 4] {
    [
        f32::from(bytes[0]) / 255.0,
        f32::from(bytes[1]) / 255.0,
        f32::from(bytes[2]) / 255.0,
        f32::from(bytes[3]) / 255.0,
    ]
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "channels are clamped to 0..1 before scaling to 0..255"
)]
fn to_bytes([r, g, b, a]: [f32; 4]) -> [u8; 4] {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    [channel(r), channel(g), channel(b), channel(a)]
}

fn serialized_srgb([r, g, b, a]: [f32; 4]) -> String {
    let alpha = if a < 1.0 {
        format!(" / {}", format_number(a))
    } else {
        String::new()
    };
    format!(
        "color(srgb {} {} {}{alpha})",
        format_number(r),
        format_number(g),
        format_number(b)
    )
}

/// A `color()` or `lab()` component: a number or the `none` keyword
/// (<https://drafts.csswg.org/css-color-4/#none-color>).
#[derive(Clone, Copy)]
enum Component {
    None,
    Number(f32),
}

impl Component {
    fn parse(text: &str) -> Option<Self> {
        if text.eq_ignore_ascii_case("none") {
            return Some(Self::None);
        }
        number(text).map(Self::Number)
    }

    fn value(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Number(value) => value,
        }
    }

    fn number(self) -> Option<f32> {
        match self {
            Self::None => None,
            Self::Number(value) => Some(value),
        }
    }
}

/// Serializes a `color()` function. Components are not clamped, `none` is
/// preserved, and alpha is clamped to [0, 1] and omitted when opaque
/// (<https://drafts.csswg.org/css-color-4/#serializing-color-function>).
fn serialize_color_function(space: &str, components: &[Component], alpha: f32) -> String {
    let component = |value: Component| match value {
        Component::None => "none".to_owned(),
        Component::Number(value) => format_number(value),
    };
    let alpha = alpha.clamp(0.0, 1.0);
    let alpha_text = if (alpha - 1.0).abs() < f32::EPSILON {
        String::new()
    } else {
        format!(" / {}", format_number(alpha))
    };
    format!(
        "color({space} {} {} {}{alpha_text})",
        component(components[0]),
        component(components[1]),
        component(components[2]),
    )
}

fn parse_color_function(inner: &str) -> Option<ParsedColor> {
    let inner = inner.trim();
    let (space, rest) = inner.split_once(char::is_whitespace)?;
    let space = space.trim();
    if space.is_empty() {
        return None;
    }
    let (channels, alpha) = rest
        .split_once('/')
        .map_or((rest, None), |(channels, alpha)| {
            (channels.trim(), Some(alpha.trim()))
        });
    let components: Vec<Component> = channels
        .split_whitespace()
        .map(Component::parse)
        .collect::<Option<_>>()?;
    if components.len() != 3 {
        return None;
    }
    let alpha_value = match alpha {
        None => 1.0,
        Some(text) => Component::parse(text)?.number().unwrap_or(1.0),
    };
    // sRGB is the only space the bitmap stores directly. Other spaces keep
    // their serialization and paint with a best-effort sRGB approximation.
    let value = [
        components[0].value().clamp(0.0, 1.0),
        components[1].value().clamp(0.0, 1.0),
        components[2].value().clamp(0.0, 1.0),
        alpha_value.clamp(0.0, 1.0),
    ];
    Some(ParsedColor {
        bytes: to_bytes(value),
        value,
        serialized: Some(serialize_color_function(space, &components, alpha_value)),
    })
}

/// A `lab()` lightness: a number, a percentage of 100, or `none`.
fn lab_lightness(text: &str) -> Option<f32> {
    if text.eq_ignore_ascii_case("none") {
        return Some(0.0);
    }
    let text = text.strip_suffix('%').unwrap_or(text);
    number(text)
}

/// A `lab()` a/b axis: a number, a percentage of 125, or `none`.
fn lab_axis(text: &str) -> Option<f32> {
    if text.eq_ignore_ascii_case("none") {
        return Some(0.0);
    }
    if let Some(percent) = text.strip_suffix('%') {
        number(percent).map(|value| value * 1.25)
    } else {
        number(text)
    }
}

fn serialize_lab(lightness: f32, a: f32, b: f32, alpha: f32) -> String {
    let alpha = alpha.clamp(0.0, 1.0);
    let alpha_text = if (alpha - 1.0).abs() < f32::EPSILON {
        String::new()
    } else {
        format!(" / {}", format_number(alpha))
    };
    format!(
        "lab({} {} {}{alpha_text})",
        format_number(lightness),
        format_number(a),
        format_number(b)
    )
}

fn parse_lab(inner: &str) -> Option<ParsedColor> {
    let (body, alpha) = inner
        .split_once('/')
        .map_or((inner, None), |(body, alpha)| {
            (body.trim(), Some(alpha.trim()))
        });
    let parts: Vec<&str> = body.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }
    let lightness = lab_lightness(parts[0])?;
    let a = lab_axis(parts[1])?;
    let b = lab_axis(parts[2])?;
    let alpha_value = alpha.map_or(1.0, |text| {
        Component::parse(text).and_then(Component::number).unwrap_or(1.0)
    });
    Some(ParsedColor {
        bytes: lab_to_srgb(lightness, a, b, alpha_value),
        value: [lightness, a, b, alpha_value],
        serialized: Some(serialize_lab(lightness, a, b, alpha_value)),
    })
}

/// Converts CIE Lab (D50) to sRGB bytes
/// (<https://drafts.csswg.org/css-color-4/#lab-to-xyz>).
fn lab_to_srgb(lightness: f32, a_axis: f32, b_axis: f32, alpha: f32) -> [u8; 4] {
    let fy = (lightness + 16.0) / 116.0;
    let fx = fy + a_axis / 500.0;
    let fz = fy - b_axis / 200.0;
    let inverse = |value: f32| {
        let cube = value * value * value;
        if cube > 216.0 / 24389.0 {
            cube
        } else {
            (116.0 * value - 16.0) / (24389.0 / 27.0)
        }
    };
    // D50 white point.
    let x_d50 = inverse(fx) * 0.964_22;
    let y_d50 = inverse(fy);
    let z_d50 = inverse(fz) * 0.825_21;
    // Bradford-adapted D50 -> D65.
    let x65 = 0.955_47 * x_d50 - 0.023_099 * y_d50 + 0.063_259 * z_d50;
    let y65 = -0.028_37 * x_d50 + 1.009_995 * y_d50 + 0.021_041 * z_d50;
    let z65 = 0.012_314 * x_d50 - 0.020_508 * y_d50 + 1.330_366 * z_d50;
    // XYZ D65 -> linear sRGB.
    let linear = [
        3.240_97 * x65 - 1.537_38 * y65 - 0.498_611 * z65,
        -0.969_244 * x65 + 1.875_97 * y65 + 0.041_555 * z65,
        0.055_63 * x65 - 0.203_977 * y65 + 1.056_97 * z65,
    ];
    let encode = |value: f32| {
        if value <= 0.003_130_8 {
            12.92 * value
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        }
    };
    to_bytes([
        encode(linear[0]),
        encode(linear[1]),
        encode(linear[2]),
        alpha,
    ])
}

fn parse_relative(rest: &str, color_function: bool) -> Option<ParsedColor> {
    let (source, channels) = split_source(rest)?;
    let source = parse_exact(source)
        .or_else(|| parse(source).map(|bytes| ParsedColor {
            bytes,
            value: from_bytes(bytes),
            serialized: None,
        }))?;
    let source = source.value;
    let channels = if color_function {
        channels.trim().strip_prefix("srgb")?.trim()
    } else {
        channels
    };
    let (channels, alpha) = channels.split_once('/').map_or((channels, None), |(channels, alpha)| {
        (channels.trim(), Some(alpha.trim()))
    });
    let evaluate = |token: &str, fallback: f32| -> f32 {
        match token {
            "r" => source[0],
            "g" => source[1],
            "b" => source[2],
            "alpha" => source[3],
            _ => token.parse::<f32>().unwrap_or(fallback).clamp(0.0, 1.0),
        }
    };
    if channels.contains(')') || channels.contains(',') {
        return None;
    }
    let tokens: Vec<&str> = channels.split_whitespace().collect();
    let valid = |token: &str| {
        matches!(token, "r" | "g" | "b" | "alpha")
            || token.trim_end_matches('%').parse::<f32>().is_ok()
    };
    if tokens.len() > 3 || tokens.iter().any(|token| !valid(token)) {
        return None;
    }
    if alpha.is_some_and(|alpha| !valid(alpha) || alpha.contains(char::is_whitespace)) {
        return None;
    }
    let value = [
        evaluate(tokens.first().copied().unwrap_or("r"), 0.0),
        evaluate(tokens.get(1).copied().unwrap_or("g"), 0.0),
        evaluate(tokens.get(2).copied().unwrap_or("b"), 0.0),
        alpha.map_or(source[3], |alpha| evaluate(alpha, source[3])),
    ];
    Some(ParsedColor {
        bytes: to_bytes(value),
        value,
        serialized: Some(serialized_srgb(value)),
    })
}

fn parse_color_mix(inner: &str) -> Option<ParsedColor> {
    let inner = inner.trim().strip_prefix("in srgb")?.trim_start_matches(',').trim();
    let parts = split_commas(inner);
    if parts.len() != 2 {
        return None;
    }
    let split = |part: &str| -> Option<(ParsedColor, Option<f32>)> {
        let part = part.trim();
        let (color, percentage) = part.rsplit_once(char::is_whitespace).map_or((part, None), |(color, percentage)| {
            if percentage.trim().ends_with('%') {
                (
                    color.trim(),
                    percentage.trim().trim_end_matches('%').parse::<f32>().ok(),
                )
            } else {
                (part, None)
            }
        });
        let color = parse_exact(color).or_else(|| {
            parse(color).map(|bytes| ParsedColor {
                bytes,
                value: from_bytes(bytes),
                serialized: None,
            })
        })?;
        Some((color, percentage))
    };
    let (first, first_weight) = split(parts[0])?;
    let (second, second_weight) = split(parts[1])?;
    let (first_weight, second_weight) = match (first_weight, second_weight) {
        (None, None) => (50.0, 50.0),
        (Some(weight), None) => (weight, 100.0 - weight),
        (None, Some(weight)) => (100.0 - weight, weight),
        (Some(first), Some(second)) => (first, second),
    };
    let total = first_weight + second_weight;
    if total <= 0.0 {
        return None;
    }
    let (first, second) = (first.value, second.value);
    let weight = first_weight / total;
    let mix = |index: usize| first[index] * weight + second[index] * (1.0 - weight);
    let value = [mix(0), mix(1), mix(2), mix(3)];
    Some(ParsedColor {
        bytes: to_bytes(value),
        value,
        serialized: Some(serialized_srgb(value)),
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn hex_lengths() {
        assert_eq!(parse("#f00"), Some([255, 0, 0, 255]));
        assert_eq!(parse("#ff0000"), Some([255, 0, 0, 255]));
        assert_eq!(parse("#ff000080"), Some([255, 0, 0, 128]));
        assert_eq!(parse("#f008"), Some([255, 0, 0, 136]));
    }

    #[test]
    fn functional_forms() {
        assert_eq!(parse("rgb(0, 128, 255)"), Some([0, 128, 255, 255]));
        assert_eq!(parse("rgba(0, 0, 0, 0.5)"), Some([0, 0, 0, 128]));
        assert_eq!(parse("rgb(100% 0% 0% / 50%)"), Some([255, 0, 0, 128]));
        assert_eq!(parse("hsl(120, 100%, 25%)"), Some([0, 128, 0, 255]));
    }

    #[test]
    fn names_and_invalid() {
        assert_eq!(parse("RED"), Some([255, 0, 0, 255]));
        assert_eq!(parse("transparent"), Some([0, 0, 0, 0]));
        assert_eq!(parse("not-a-color"), None);
        assert_eq!(parse("#12"), None);
    }
}
