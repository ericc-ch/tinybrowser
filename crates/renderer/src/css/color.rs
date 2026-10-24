//! CSS `<color>` parsing for canvas styles and the input color state.
//!
//! Both consumers accept any CSS color where they take a color
//! (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-fillstyle>,
//! <https://html.spec.whatwg.org/multipage/input.html#color-state-(type=color)>).
//! The parser follows CSS Color 4: named and system colors, hex, `rgb()`,
//! `hsl()`, `hwb()`, `lab()`, relative colors, `color-mix()`, and every
//! predefined `color()` space. It serializes the authored syntax for canvas
//! when the sRGB bytes cannot represent it.
//!
//! Known gap: out-of-gamut colors are clipped per channel instead of running
//! the CSS gamut-mapping algorithm, so wide-gamut values that are far outside
//! sRGB can differ from a browser that implements OKLCH local-MINDE mapping.

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

fn percentage_or_number(text: &str) -> Option<f32> {
    if text.eq_ignore_ascii_case("none") {
        return Some(0.0);
    }
    percentage(text).or_else(|| number(text).map(|value| value / 100.0))
}

fn alpha(text: Option<&str>) -> Option<u8> {
    let Some(text) = text else { return Some(255) };
    if text.eq_ignore_ascii_case("none") {
        return Some(0);
    }
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
    if text.eq_ignore_ascii_case("none") {
        return Some(0.0);
    }
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
    if text.eq_ignore_ascii_case("none") {
        return Some(0.0);
    }
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
    // The modern syntax accepts a bare number for saturation and lightness,
    // where 100 means 100%; the legacy comma syntax requires percentages
    // (<https://drafts.csswg.org/css-color-4/#the-hsl-notation>).
    let percentage_channel = |text: &str| {
        if args.comma {
            percentage(text)
        } else {
            percentage_or_number(text)
        }
    };
    let saturation = percentage_channel(&components[1])?.clamp(0.0, 1.0);
    let lightness = percentage_channel(&components[2])?.clamp(0.0, 1.0);
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
    let percentage_channel = |text: &str| {
        if text.eq_ignore_ascii_case("none") {
            Some(0.0)
        } else {
            percentage(text)
        }
    };
    let whiteness = percentage_channel(&components[1])?.clamp(0.0, 1.0);
    let blackness = percentage_channel(&components[2])?.clamp(0.0, 1.0);
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
    // CSS system colors resolve to the UA's light palette. tinybrowser has
    // no platform theme, so it uses the Chrome light values, including the
    // 19 standard names and their legacy aliases
    // (<https://drafts.csswg.org/css-color-4/#css-system-colors>).
    let palette =
        match name {
            "accentcolor" => [0x00, 0x75, 0xff, 255],
            "accentcolortext" | "highlighttext" | "selecteditemtext" | "canvas" | "field"
            | "window" | "background" | "infobackground" | "menu" | "appworkspace"
            | "scrollbar" => [0xff, 0xff, 0xff, 255],
            "activetext" => [0xff, 0x00, 0x00, 255],
            "buttonborder" | "buttontext" | "canvastext" | "captiontext" | "fieldtext"
            | "infotext" | "marktext" | "menutext" | "windowtext" | "inactiveborder"
            | "threeddarkshadow" | "threedhighlight" | "threedlightshadow" | "threedshadow"
            | "windowframe" => [0, 0, 0, 255],
            "buttonface" | "threedface" | "activecaption" | "inactivecaption"
            | "buttonhighlight" | "buttonshadow" => [0xef, 0xef, 0xef, 255],
            "graytext" | "inactivecaptiontext" | "activeborder" => [0x80, 0x80, 0x80, 255],
            "highlight" => [0x00, 0x41, 0xc6, 204],
            "linktext" => [0x00, 0x00, 0xee, 255],
            "visitedtext" => [0x55, 0x1a, 0x8b, 255],
            "mark" => [0xff, 0xff, 0x00, 255],
            "selecteditem" => [0x19, 0x67, 0xd2, 255],
            _ => return None,
        };
    Some(palette)
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
    if text == "-0" { "0".to_owned() } else { text }
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
pub(crate) fn to_bytes([r, g, b, a]: [f32; 4]) -> [u8; 4] {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    [channel(r), channel(g), channel(b), channel(a)]
}

fn serialized_srgb([r, g, b, a]: [f32; 4]) -> String {
    let alpha = alpha_suffix(a);
    format!(
        "color(srgb {} {} {}{alpha})",
        format_number(r),
        format_number(g),
        format_number(b)
    )
}

/// Formats the `/ alpha` suffix for a serialized color. Every caller clamps
/// first, so an opaque color is exactly `1.0`
/// (<https://drafts.csswg.org/css-color-4/#serializing-color-values>).
fn alpha_suffix(alpha: f32) -> String {
    if alpha >= 1.0 {
        String::new()
    } else {
        format!(" / {}", format_number(alpha))
    }
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
    let alpha_text = alpha_suffix(alpha);
    format!(
        "color({space} {} {} {}{alpha_text})",
        component(components[0]),
        component(components[1]),
        component(components[2]),
    )
}

/// The authored Display P3 components and alpha of a `color(display-p3 …)`
/// value, without clamping or converting spaces. A color input in the
/// `display-p3` color space keeps these components, including values outside
/// the 0..1 range
/// (<https://html.spec.whatwg.org/multipage/input.html#update-a-color-well-control-color>).
pub(crate) fn display_p3_components(input: &str) -> Option<([f32; 3], f32)> {
    let lower = input.trim().to_ascii_lowercase();
    let inner = function_args(&lower, "color")?;
    let (space, rest) = inner.trim().split_once(char::is_whitespace)?;
    if !space.eq_ignore_ascii_case("display-p3") {
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
    let alpha = match alpha {
        None => 1.0,
        Some(text) => Component::parse(text)?.number()?,
    };
    Some((
        [
            components[0].value(),
            components[1].value(),
            components[2].value(),
        ],
        alpha,
    ))
}

fn parse_color_function(inner: &str) -> Option<ParsedColor> {
    let inner = inner.trim();
    let (space, rest) = inner.split_once(char::is_whitespace)?;
    let space = space.trim();
    // `color()` accepts only the predefined color spaces; an unknown identifier
    // makes the whole declaration invalid
    // (<https://drafts.csswg.org/css-color-4/#color-function>).
    if !matches!(
        space,
        "srgb"
            | "srgb-linear"
            | "display-p3"
            | "a98-rgb"
            | "prophoto-rgb"
            | "rec2020"
            | "xyz"
            | "xyz-d50"
            | "xyz-d65"
    ) {
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
        Some(text) => Component::parse(text)?.number()?,
    };
    let srgb = color_space_to_srgb(
        space,
        [
            components[0].value(),
            components[1].value(),
            components[2].value(),
        ],
    );
    let bytes = to_bytes([srgb[0], srgb[1], srgb[2], alpha_value.clamp(0.0, 1.0)]);
    Some(ParsedColor {
        bytes,
        // Keep the converted float components, so a color input's value is
        // not quantized to 8-bit sRGB before it serializes again
        // (<https://html.spec.whatwg.org/multipage/input.html#color-state-(type=color)>).
        value: [srgb[0], srgb[1], srgb[2], alpha_value.clamp(0.0, 1.0)],
        serialized: Some(serialize_color_function(space, &components, alpha_value)),
    })
}

/// Converts a predefined `color()` space to non-linear sRGB
/// (<https://drafts.csswg.org/css-color-4/#color-conversion-code>).
///
/// Out-of-gamut values are clipped per channel when they become bytes. CSS
/// Color 4 specifies OKLCH local-MINDE gamut mapping first; that step is a
/// recorded gap, so wide-gamut values that are far outside sRGB can differ
/// from a browser that implements it.
fn color_space_to_srgb(space: &str, components: [f32; 3]) -> [f32; 3] {
    match space {
        "srgb-linear" => components.map(gamma_encode),
        "display-p3" => xyz_d65_to_srgb(matrix(DISPLAY_P3_TO_XYZ_D65, components.map(linearize))),
        "a98-rgb" => xyz_d65_to_srgb(matrix(A98_RGB_TO_XYZ_D65, components.map(a98_decode))),
        "prophoto-rgb" => xyz_d50_to_srgb(matrix(
            PROPHOTO_RGB_TO_XYZ_D50,
            components.map(prophoto_decode),
        )),
        "rec2020" => xyz_d65_to_srgb(matrix(REC2020_TO_XYZ_D65, components.map(rec2020_decode))),
        "xyz" | "xyz-d65" => xyz_d65_to_srgb(components),
        "xyz-d50" => xyz_d50_to_srgb(components),
        _ => components,
    }
}

/// Converts non-linear sRGB to non-linear Display P3, for serializing a
/// color input's value in the `display-p3` color space
/// (<https://drafts.csswg.org/css-color-4/#color-conversion-code>).
pub(crate) fn srgb_to_display_p3(components: [f32; 3]) -> [f32; 3] {
    let xyz = matrix(SRGB_TO_XYZ_D65, components.map(linearize));
    matrix(XYZ_D65_TO_DISPLAY_P3, xyz).map(gamma_encode)
}

fn matrix(coefficients: [[f32; 3]; 3], [x, y, z]: [f32; 3]) -> [f32; 3] {
    coefficients.map(|row| row[0] * x + row[1] * y + row[2] * z)
}

fn xyz_d65_to_srgb(xyz: [f32; 3]) -> [f32; 3] {
    matrix(XYZ_D65_TO_SRGB, xyz).map(gamma_encode)
}

fn xyz_d50_to_srgb(xyz: [f32; 3]) -> [f32; 3] {
    xyz_d65_to_srgb(matrix(BRADFORD_D50_TO_D65, xyz))
}

fn a98_decode(value: f32) -> f32 {
    value.signum() * value.abs().powf(563.0 / 256.0)
}

fn prophoto_decode(value: f32) -> f32 {
    value.signum() * value.abs().powf(1.8)
}

fn rec2020_decode(value: f32) -> f32 {
    const ALPHA: f32 = 1.099_296_8;
    const BETA: f32 = 0.018_054;
    if value.abs() < BETA * 4.5 {
        value / 4.5
    } else {
        value.signum() * ((value.abs() + ALPHA - 1.0) / ALPHA).powf(1.0 / 0.45)
    }
}

static DISPLAY_P3_TO_XYZ_D65: [[f32; 3]; 3] = [
    [0.486_570_95, 0.265_667_7, 0.198_217_28],
    [0.228_974_56, 0.691_738_5, 0.079_286_91],
    [0.0, 0.045_113_38, 1.043_944_4],
];

static A98_RGB_TO_XYZ_D65: [[f32; 3]; 3] = [
    [0.576_669_04, 0.185_558_24, 0.188_228_65],
    [0.297_344_98, 0.627_363_57, 0.075_291_46],
    [0.027_031_36, 0.070_688_85, 0.991_337_54],
];

static PROPHOTO_RGB_TO_XYZ_D50: [[f32; 3]; 3] = [
    [0.797_760_5, 0.135_185_84, 0.031_349_35],
    [0.288_071_13, 0.711_843_2, 0.000_085_65],
    [0.0, 0.0, 0.825_104_6],
];

static REC2020_TO_XYZ_D65: [[f32; 3]; 3] = [
    [0.636_958_05, 0.144_616_9, 0.168_880_98],
    [0.262_700_2, 0.677_998_1, 0.059_301_72],
    [0.0, 0.028_072_693, 1.060_985_1],
];

static BRADFORD_D50_TO_D65: [[f32; 3]; 3] = [
    [0.955_473_5, -0.023_098_54, 0.063_259_31],
    [-0.028_369_707, 1.009_995_5, 0.021_041_399],
    [0.012_314_002, -0.020_507_697, 1.330_365_9],
];

static SRGB_TO_XYZ_D65: [[f32; 3]; 3] = [
    [0.412_390_8, 0.357_584_3, 0.180_480_8],
    [0.212_639, 0.715_168_7, 0.072_192_31],
    [0.019_330_819, 0.119_194_78, 0.950_532_15],
];

static XYZ_D65_TO_DISPLAY_P3: [[f32; 3]; 3] = [
    [2.493_497, -0.931_383_6, -0.402_710_78],
    [-0.829_489, 1.762_664_1, 0.023_624_69],
    [0.035_845_83, -0.076_172_39, 0.956_884_5],
];

static XYZ_D65_TO_SRGB: [[f32; 3]; 3] = [
    [3.240_97, -1.537_383_2, -0.498_610_76],
    [-0.969_243_65, 1.875_967_5, 0.041_555_06],
    [0.055_630_08, -0.203_976_96, 1.056_971_5],
];

fn linearize(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn gamma_encode(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
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
    let alpha_text = alpha_suffix(alpha);
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
    let lightness = lab_lightness(parts[0])?.clamp(0.0, 100.0);
    let a = lab_axis(parts[1])?;
    let b = lab_axis(parts[2])?;
    let alpha_value = match alpha {
        None => 1.0,
        Some(text) => Component::parse(text)?.number()?,
    };
    let bytes = lab_to_srgb(lightness, a, b, alpha_value);
    Some(ParsedColor {
        bytes,
        // `value` is sRGB 0..1 everywhere else, so store the converted color
        // rather than raw Lab coordinates. `serialized` keeps the Lab syntax.
        value: from_bytes(bytes),
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
    // Bradford-adapted D50 -> the shared D65 -> sRGB conversion path.
    let srgb = xyz_d50_to_srgb([x_d50, y_d50, z_d50]);
    to_bytes([srgb[0], srgb[1], srgb[2], alpha])
}

fn parse_relative(rest: &str, color_function: bool) -> Option<ParsedColor> {
    let (source, channels) = split_source(rest)?;
    let source = parse_exact(source).or_else(|| {
        parse(source).map(|bytes| ParsedColor {
            bytes,
            value: from_bytes(bytes),
            serialized: None,
        })
    })?;
    let source = source.value;
    let channels = if color_function {
        channels.trim().strip_prefix("srgb")?.trim()
    } else {
        channels
    };
    let (channels, alpha) = channels
        .split_once('/')
        .map_or((channels, None), |(channels, alpha)| {
            (channels.trim(), Some(alpha.trim()))
        });
    let evaluate = |token: &str| -> Option<f32> {
        if token.eq_ignore_ascii_case("none") {
            return Some(0.0);
        }
        let channel = match token {
            "r" => Some(source[0]),
            "g" => Some(source[1]),
            "b" => Some(source[2]),
            "alpha" => Some(source[3]),
            _ => None,
        };
        if channel.is_some() {
            return channel;
        }
        let (number_text, percent) = token
            .strip_suffix('%')
            .map_or((token, false), |text| (text, true));
        let number = number_text.parse::<f32>().ok()?;
        let scaled = if percent {
            number / 100.0
        } else if color_function {
            number
        } else {
            // `rgb(from …)` numbers are 0..255; `color(from …)` numbers are
            // already 0..1 (<https://drafts.csswg.org/css-color-5/#relative-colors>).
            number / 255.0
        };
        Some(scaled.clamp(0.0, 1.0))
    };
    if channels.contains(')') || channels.contains(',') {
        return None;
    }
    let tokens: Vec<&str> = channels.split_whitespace().collect();
    if tokens.len() > 3 {
        return None;
    }
    let mut value = [source[0], source[1], source[2], source[3]];
    for (position, token) in tokens.iter().enumerate() {
        value[position] = evaluate(token)?;
    }
    if let Some(alpha) = alpha {
        if alpha.contains(char::is_whitespace) {
            return None;
        }
        value[3] = evaluate(alpha)?;
    }
    Some(ParsedColor {
        bytes: to_bytes(value),
        value,
        serialized: Some(serialized_srgb(value)),
    })
}

fn parse_color_mix(inner: &str) -> Option<ParsedColor> {
    let inner = inner
        .trim()
        .strip_prefix("in srgb")?
        .trim_start_matches(',')
        .trim();
    let parts = split_commas(inner);
    if parts.len() != 2 {
        return None;
    }
    let split = |part: &str| -> Option<(ParsedColor, Option<f32>)> {
        let part = part.trim();
        let mut color = part;
        let mut percentage = None;
        if let Some((head, tail)) = part.rsplit_once(char::is_whitespace) {
            let tail = tail.trim();
            if tail.ends_with('%') {
                let value = tail.trim_end_matches('%').parse::<f32>().ok()?;
                color = head.trim();
                percentage = Some(value);
            }
        }
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
    // Percentages are limited to 0..100; a missing one takes what is left.
    if first_weight.is_some_and(|weight| !(0.0..=100.0).contains(&weight))
        || second_weight.is_some_and(|weight| !(0.0..=100.0).contains(&weight))
    {
        return None;
    }
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
    // When the weights add up to less than 100%, the remaining percentage is
    // transparent, so the result's alpha is multiplied by the sum
    // (<https://drafts.csswg.org/css-color-5/#color-mix-result>).
    let alpha_multiplier = if total < 100.0 { total / 100.0 } else { 1.0 };
    let (first, second) = (first.value, second.value);
    let weight = first_weight / total;
    let other = 1.0 - weight;
    // Interpolation is on premultiplied components; mixing straight values
    // turns `transparent` into a darkened color instead of a translucent one.
    let alpha = first[3] * weight + second[3] * other;
    let mix = |index: usize| {
        let premultiplied = first[index] * first[3] * weight + second[index] * second[3] * other;
        if alpha > 0.0 {
            premultiplied / alpha
        } else {
            0.0
        }
    };
    let value = [
        mix(0),
        mix(1),
        mix(2),
        (alpha * alpha_multiplier).clamp(0.0, 1.0),
    ];
    Some(ParsedColor {
        bytes: to_bytes(value),
        value,
        serialized: Some(serialized_srgb(value)),
    })
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_exact};

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

    #[test]
    fn color_function_rejects_unknown_spaces_and_bad_alpha() {
        assert!(parse_exact("color(bogus 1 0 0)").is_none());
        assert!(parse_exact("color(srgb 1 0 0 / garbage)").is_none());
        assert!(parse_exact("lab(50 0 0 / garbage)").is_none());
        assert!(parse_exact("lab(50 0 0)").is_some());
    }

    #[test]
    fn color_mix_is_premultiplied_and_scales_partial_percentages() {
        let transparent_red = parse_exact("color-mix(in srgb, transparent, red)").unwrap();
        assert_eq!(transparent_red.bytes, [255, 0, 0, 128]);

        let partial = parse_exact("color-mix(in srgb, red 25%, blue 25%)").unwrap();
        assert_eq!(partial.bytes, [128, 0, 128, 128]);

        assert!(parse_exact("color-mix(in srgb, red 150%, blue)").is_none());
        assert!(parse_exact("color-mix(in srgb, red -1%, blue)").is_none());
    }

    #[test]
    fn relative_rgb_numbers_are_zero_to_255() {
        let mixed = parse_exact("rgb(from #000000 128 0 0)").unwrap();
        assert_eq!(mixed.bytes, [128, 0, 0, 255]);
    }

    #[test]
    fn lab_mixes_in_srgb_not_lab_coordinates() {
        // The sRGB midpoint of black and white is [128, 128, 128]; mixing the
        // lab coordinates themselves would give L=50, [119, 119, 119].
        let mixed = parse_exact("color-mix(in srgb, lab(0 0 0), lab(100 0 0))").unwrap();
        assert_eq!(mixed.bytes, [128, 128, 128, 255]);
    }
}
