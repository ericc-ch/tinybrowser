//! CSS colors: named colors, hex, and the `rgb()`/`rgba()` function forms.
//!
//! Grammar and the named table follow CSS Color 4
//! (<https://drafts.csswg.org/css-color-4/#color-syntax> and
//! <https://drafts.csswg.org/css-color-4/#named-colors>). `currentColor`,
//! `inherit`, and the global keywords are resolved by the cascade, not here.

/// A straight-alpha 8-bit sRGB color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel; 255 is opaque.
    pub a: u8,
}

impl Color {
    /// Opaque black, the initial value of `color`.
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    /// Opaque white, the canvas background in the absence of a page color.
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    /// Fully transparent.
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    /// An opaque color.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// A color with explicit alpha.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Premultiplied `[r, g, b, a]` for blending into a premultiplied buffer.
    #[must_use]
    pub(crate) fn premultiplied(self) -> [u8; 4] {
        let alpha = u32::from(self.a);
        [
            u8_from((u32::from(self.r) * alpha + 127) / 255),
            u8_from((u32::from(self.g) * alpha + 127) / 255),
            u8_from((u32::from(self.b) * alpha + 127) / 255),
            self.a,
        ]
    }

    /// Parses one CSS color value.
    ///
    /// `None` means the token is not a color this subset implements (including
    /// `currentColor`, which the cascade resolves before reaching paint).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if let Some(hex) = value.strip_prefix('#') {
            return parse_hex(hex);
        }
        let lower = value.to_ascii_lowercase();
        if let Some(color) = named(&lower) {
            return Some(color);
        }
        if let Some(args) = function_args(&lower, "rgb").or_else(|| function_args(&lower, "rgba")) {
            return parse_rgb_function(args);
        }
        None
    }
}

/// Truncating channel helper: inputs are always in `0..=255` after clamping.
fn u8_from(value: u32) -> u8 {
    u8::try_from(value.min(255)).unwrap_or(255)
}

fn parse_hex(hex: &str) -> Option<Color> {
    let digits: Vec<u8> = hex
        .chars()
        .map(|ch| ch.to_digit(16).and_then(|digit| u8::try_from(digit).ok()))
        .collect::<Option<Vec<u8>>>()?;
    match digits.len() {
        3 => Some(Color::rgb(
            digits[0] * 17,
            digits[1] * 17,
            digits[2] * 17,
        )),
        4 => Some(Color::rgba(
            digits[0] * 17,
            digits[1] * 17,
            digits[2] * 17,
            digits[3] * 17,
        )),
        6 => Some(Color::rgb(
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
        )),
        8 => Some(Color::rgba(
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
            digits[6] * 16 + digits[7],
        )),
        _ => None,
    }
}

/// Splits `name(args)` into its argument string.
fn function_args<'a>(value: &'a str, name: &str) -> Option<&'a str> {
    let rest = value.strip_prefix(name)?.strip_prefix('(')?;
    rest.strip_suffix(')')
}

/// Parses the comma or space separated components of `rgb()`/`rgba()`.
fn parse_rgb_function(args: &str) -> Option<Color> {
    let (channels, alpha) = match args.split_once('/') {
        Some((channels, alpha)) => (channels, Some(alpha)),
        None => (args, None),
    };
    let mut components: Vec<&str> = Vec::new();
    if channels.contains(',') {
        for part in channels.split(',') {
            components.push(part.trim());
        }
    } else {
        for part in channels.split_whitespace() {
            components.push(part);
        }
    }
    // The legacy `rgba(r, g, b, a)` form carries alpha as a fourth comma
    // separator component (<https://drafts.csswg.org/css-color-4/#rgb-functions>).
    let (components, alpha) = match (components.len(), alpha) {
        (4, None) => (components[..3].to_vec(), Some(components[3])),
        _ => (components, alpha),
    };
    if components.len() != 3 {
        return None;
    }
    let r = parse_channel(components[0])?;
    let g = parse_channel(components[1])?;
    let b = parse_channel(components[2])?;
    let a = match alpha {
        Some(alpha) => parse_alpha(alpha.trim())?,
        None => 255,
    };
    Some(Color::rgba(r, g, b, a))
}

/// `rgb()` channels are `<number>` or `<percentage>`
/// (<https://drafts.csswg.org/css-color-4/#rgb-functions>), with commas only
/// in the legacy form; both forms are normalized to 0..=255.
fn parse_channel(component: &str) -> Option<u8> {
    if let Some(percent) = component.strip_suffix('%') {
        let percent: f32 = percent.trim().parse().ok()?;
        let clamped = percent.clamp(0.0, 100.0);
        Some(channel_from_float(clamped * 2.55))
    } else {
        let number: f32 = component.parse().ok()?;
        Some(channel_from_float(number))
    }
}

fn channel_from_float(value: f32) -> u8 {
    let rounded = (value + 0.5).floor().clamp(0.0, 255.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded is finite and clamped to 0..=255"
    )]
    let channel = rounded as u32;
    u8_from(channel)
}

/// Alpha is `<number>` or `<percentage>` and clamps to 0..=1.
fn parse_alpha(component: &str) -> Option<u8> {
    let alpha = if let Some(percent) = component.strip_suffix('%') {
        percent.trim().parse::<f32>().ok()? / 100.0
    } else {
        component.parse::<f32>().ok()?
    };
    Some(alpha_from_float(alpha))
}

/// Converts a `0..=1` alpha to 8 bits.
fn alpha_from_float(alpha: f32) -> u8 {
    let rounded = (alpha.clamp(0.0, 1.0) * 255.0 + 0.5).floor();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded is finite and clamped to 0..=255"
    )]
    let value = rounded as u32;
    u8_from(value)
}

/// The CSS named color table (<https://drafts.csswg.org/css-color-4/#named-colors>),
/// sorted for binary search.
const NAMED: &[(&str, (u8, u8, u8))] = &[
    ("aliceblue", (240, 248, 255)),
    ("antiquewhite", (250, 235, 215)),
    ("aqua", (0, 255, 255)),
    ("aquamarine", (127, 255, 212)),
    ("azure", (240, 255, 255)),
    ("beige", (245, 245, 220)),
    ("bisque", (255, 228, 196)),
    ("black", (0, 0, 0)),
    ("blanchedalmond", (255, 235, 205)),
    ("blue", (0, 0, 255)),
    ("blueviolet", (138, 43, 226)),
    ("brown", (165, 42, 42)),
    ("burlywood", (222, 184, 135)),
    ("cadetblue", (95, 158, 160)),
    ("chartreuse", (127, 255, 0)),
    ("chocolate", (210, 105, 30)),
    ("coral", (255, 127, 80)),
    ("cornflowerblue", (100, 149, 237)),
    ("cornsilk", (255, 248, 220)),
    ("crimson", (220, 20, 60)),
    ("cyan", (0, 255, 255)),
    ("darkblue", (0, 0, 139)),
    ("darkcyan", (0, 139, 139)),
    ("darkgoldenrod", (184, 134, 11)),
    ("darkgray", (169, 169, 169)),
    ("darkgreen", (0, 100, 0)),
    ("darkgrey", (169, 169, 169)),
    ("darkkhaki", (189, 183, 107)),
    ("darkmagenta", (139, 0, 139)),
    ("darkolivegreen", (85, 107, 47)),
    ("darkorange", (255, 140, 0)),
    ("darkorchid", (153, 50, 204)),
    ("darkred", (139, 0, 0)),
    ("darksalmon", (233, 150, 122)),
    ("darkseagreen", (143, 188, 143)),
    ("darkslateblue", (72, 61, 139)),
    ("darkslategray", (47, 79, 79)),
    ("darkslategrey", (47, 79, 79)),
    ("darkturquoise", (0, 206, 209)),
    ("darkviolet", (148, 0, 211)),
    ("deeppink", (255, 20, 147)),
    ("deepskyblue", (0, 191, 255)),
    ("dimgray", (105, 105, 105)),
    ("dimgrey", (105, 105, 105)),
    ("dodgerblue", (30, 144, 255)),
    ("firebrick", (178, 34, 34)),
    ("floralwhite", (255, 250, 240)),
    ("forestgreen", (34, 139, 34)),
    ("fuchsia", (255, 0, 255)),
    ("gainsboro", (220, 220, 220)),
    ("ghostwhite", (248, 248, 255)),
    ("gold", (255, 215, 0)),
    ("goldenrod", (218, 165, 32)),
    ("gray", (128, 128, 128)),
    ("green", (0, 128, 0)),
    ("greenyellow", (173, 255, 47)),
    ("grey", (128, 128, 128)),
    ("honeydew", (240, 255, 240)),
    ("hotpink", (255, 105, 180)),
    ("indianred", (205, 92, 92)),
    ("indigo", (75, 0, 130)),
    ("ivory", (255, 255, 240)),
    ("khaki", (240, 230, 140)),
    ("lavender", (230, 230, 250)),
    ("lavenderblush", (255, 240, 245)),
    ("lawngreen", (124, 252, 0)),
    ("lemonchiffon", (255, 250, 205)),
    ("lightblue", (173, 216, 230)),
    ("lightcoral", (240, 128, 128)),
    ("lightcyan", (224, 255, 255)),
    ("lightgoldenrodyellow", (250, 250, 210)),
    ("lightgray", (211, 211, 211)),
    ("lightgreen", (144, 238, 144)),
    ("lightgrey", (211, 211, 211)),
    ("lightpink", (255, 182, 193)),
    ("lightsalmon", (255, 160, 122)),
    ("lightseagreen", (32, 178, 170)),
    ("lightskyblue", (135, 206, 250)),
    ("lightslategray", (119, 136, 153)),
    ("lightslategrey", (119, 136, 153)),
    ("lightsteelblue", (176, 196, 222)),
    ("lightyellow", (255, 255, 224)),
    ("lime", (0, 255, 0)),
    ("limegreen", (50, 205, 50)),
    ("linen", (250, 240, 230)),
    ("magenta", (255, 0, 255)),
    ("maroon", (128, 0, 0)),
    ("mediumaquamarine", (102, 205, 170)),
    ("mediumblue", (0, 0, 205)),
    ("mediumorchid", (186, 85, 211)),
    ("mediumpurple", (147, 112, 219)),
    ("mediumseagreen", (60, 179, 113)),
    ("mediumslateblue", (123, 104, 238)),
    ("mediumspringgreen", (0, 250, 154)),
    ("mediumturquoise", (72, 209, 204)),
    ("mediumvioletred", (199, 21, 133)),
    ("midnightblue", (25, 25, 112)),
    ("mintcream", (245, 255, 250)),
    ("mistyrose", (255, 228, 225)),
    ("moccasin", (255, 228, 181)),
    ("navajowhite", (255, 222, 173)),
    ("navy", (0, 0, 128)),
    ("oldlace", (253, 245, 230)),
    ("olive", (128, 128, 0)),
    ("olivedrab", (107, 142, 35)),
    ("orange", (255, 165, 0)),
    ("orangered", (255, 69, 0)),
    ("orchid", (218, 112, 214)),
    ("palegoldenrod", (238, 232, 170)),
    ("palegreen", (152, 251, 152)),
    ("paleturquoise", (175, 238, 238)),
    ("palevioletred", (219, 112, 147)),
    ("papayawhip", (255, 239, 213)),
    ("peachpuff", (255, 218, 185)),
    ("peru", (205, 133, 63)),
    ("pink", (255, 192, 203)),
    ("plum", (221, 160, 221)),
    ("powderblue", (176, 224, 230)),
    ("purple", (128, 0, 128)),
    ("rebeccapurple", (102, 51, 153)),
    ("red", (255, 0, 0)),
    ("rosybrown", (188, 143, 143)),
    ("royalblue", (65, 105, 225)),
    ("saddlebrown", (139, 69, 19)),
    ("salmon", (250, 128, 114)),
    ("sandybrown", (244, 164, 96)),
    ("seagreen", (46, 139, 87)),
    ("seashell", (255, 245, 238)),
    ("sienna", (160, 82, 45)),
    ("silver", (192, 192, 192)),
    ("skyblue", (135, 206, 235)),
    ("slateblue", (106, 90, 205)),
    ("slategray", (112, 128, 144)),
    ("slategrey", (112, 128, 144)),
    ("snow", (255, 250, 250)),
    ("springgreen", (0, 255, 127)),
    ("steelblue", (70, 130, 180)),
    ("tan", (210, 180, 140)),
    ("teal", (0, 128, 128)),
    ("thistle", (216, 191, 216)),
    ("tomato", (255, 99, 71)),
    ("turquoise", (64, 224, 208)),
    ("violet", (238, 130, 238)),
    ("wheat", (245, 222, 179)),
    ("white", (255, 255, 255)),
    ("whitesmoke", (245, 245, 245)),
    ("yellow", (255, 255, 0)),
    ("yellowgreen", (154, 205, 50)),
];

/// Looks up a named color; \`transparent\` maps to zero alpha.
fn named(name: &str) -> Option<Color> {
    if name == "transparent" {
        return Some(Color::TRANSPARENT);
    }
    let (_, (r, g, b)) = *NAMED
        .binary_search_by(|(candidate, _)| candidate.cmp(&name))
        .ok()
        .and_then(|index| NAMED.get(index))?;
    Some(Color::rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::Color;

    #[test]
    fn parses_hex_forms() {
        assert_eq!(Color::parse("#f00"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(Color::parse("#ff0000"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(
            Color::parse("#ff000080"),
            Some(Color::rgba(255, 0, 0, 128))
        );
        assert_eq!(Color::parse("#ff00"), Some(Color::rgba(255, 255, 0, 0)));
        assert_eq!(Color::parse("#12345"), None);
    }

    #[test]
    fn parses_rgb_functions() {
        assert_eq!(
            Color::parse("rgb(255, 128, 0)"),
            Some(Color::rgb(255, 128, 0))
        );
        assert_eq!(
            Color::parse("rgb(100% 0% 50%)"),
            Some(Color::rgb(255, 0, 128))
        );
        assert_eq!(
            Color::parse("rgba(1, 2, 3, 0.5)"),
            Some(Color::rgba(1, 2, 3, 128))
        );
        assert_eq!(
            Color::parse("rgb(0 0 0 / 50%)"),
            Some(Color::rgba(0, 0, 0, 128))
        );
        assert_eq!(Color::parse("rgb(0 0)"), None);
    }

    #[test]
    fn parses_named_colors() {
        assert_eq!(Color::parse("White"), Some(Color::WHITE));
        assert_eq!(Color::parse("rebeccapurple"), Some(Color::rgb(102, 51, 153)));
        assert_eq!(Color::parse("transparent"), Some(Color::TRANSPARENT));
        assert_eq!(Color::parse("notacolor"), None);
    }
}
