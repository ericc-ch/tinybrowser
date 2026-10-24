//! CSS `<color>` parsing and serialization for canvas styles and the input
//! color state.
//!
//! The style system owns the grammar, so every form it accepts — named and
//! system colors, hex, `rgb()`, `hsl()`, `hwb()`, `lab()`, `lch()`, `oklab()`,
//! `oklch()`, `color()`, relative colors, and `color-mix()` — parses here with
//! the same validation the cascade uses
//! (<https://drafts.csswg.org/css-color-4/#parse-a-css-color-value>). Canvas
//! gets straight sRGB bytes for painting plus the serialization the getter
//! must report (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-fillstyle>).
//!
//! Known gap: out-of-gamut colors are clipped per channel when converted to
//! sRGB instead of running the CSS gamut-mapping algorithm, so wide-gamut
//! values far outside sRGB can differ from a browser that implements OKLCH
//! local-MINDE mapping.

use style::color::{AbsoluteColor, ColorSpace};
use style::context::QuirksMode;
use style::device::Device;
use style::parser::ParserContext;
use style::servo_arc::Arc as ServoArc;
use style::stylesheets::{CssRuleType, Origin, UrlExtraData};
use style::values::Parser;
use style::values::specified::Color as SpecifiedColor;
use style_traits::{ParsingMode, ToCss};
use url::Url;

/// A parsed color plus the exact CSS serialization, when the syntax has no
/// HTML-compatible sRGB form (`color()`, relative colors, `color-mix()`).
pub(crate) struct ParsedColor {
    pub(crate) bytes: [u8; 4],
    /// Straight (non-premultiplied) sRGB components plus alpha, each 0..=1.
    pub(crate) value: [f32; 4],
    pub(crate) serialized: Option<String>,
}

/// Parses one CSS color and computes it to an absolute color.
///
/// `device` supplies the color scheme a system color resolves against; without
/// one, a system color fails like it does in a context with no used color
/// scheme.
fn parse_absolute(input: &str, device: Option<&Device>) -> Option<AbsoluteColor> {
    let url_data = UrlExtraData(ServoArc::new(Url::parse("about:blank").ok()?));
    let context = ParserContext::new(
        Origin::Author,
        &url_data,
        Some(CssRuleType::Style),
        ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        std::borrow::Cow::Owned(style::stylesheets::Namespaces::default()),
        None,
        None,
        style::custom_properties::AttrTaint::default(),
    );
    let mut parser = Parser::new(input);
    let computed = SpecifiedColor::parse_and_compute(&context, &mut parser, device).ok()?;
    Some(computed.resolve_to_absolute(&AbsoluteColor::BLACK))
}

/// Parses a color for canvas and the color input, preserving the serialization
/// of a non-legacy color space.
pub(crate) fn parse_exact(input: &str, device: Option<&Device>) -> Option<ParsedColor> {
    let absolute = parse_absolute(input, device)?;
    let srgb = absolute.to_color_space(ColorSpace::Srgb);
    let value = [
        srgb.components.0,
        srgb.components.1,
        srgb.components.2,
        srgb.alpha,
    ];
    // Legacy sRGB serializes in the HTML-compatible short form
    // (`#rrggbb`/`rgba()`), which the canvas getter derives from the bytes.
    // Every other space serializes in its own syntax, as the canvas getter
    // requires (<https://drafts.csswg.org/css-color-4/#serializing-color-values>).
    let serialized = if absolute.is_legacy_syntax() {
        None
    } else {
        Some(absolute.to_css_string())
    };
    Some(ParsedColor {
        bytes: to_bytes(value),
        value,
        serialized,
    })
}

/// Clamps straight components in 0..=1 to bytes.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "channels are clamped to 0..1 before scaling to 0..255"
)]
pub(crate) fn to_bytes([r, g, b, a]: [f32; 4]) -> [u8; 4] {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    [channel(r), channel(g), channel(b), channel(a)]
}

/// The color's components in Display P3 plus its alpha, keeping authored
/// Display P3 components when the input already names that space.
///
/// The color input serializes this directly, so a round trip through sRGB must
/// not perturb a `color(display-p3 …)` value it was given
/// (<https://html.spec.whatwg.org/multipage/input.html#update-a-color-well-control-color>).
pub(crate) fn display_p3_components(
    input: &str,
    device: Option<&Device>,
) -> Option<([f32; 3], f32)> {
    let absolute = parse_absolute(input, device)?;
    let p3 = if absolute.color_space == ColorSpace::DisplayP3 {
        absolute
    } else {
        absolute.to_color_space(ColorSpace::DisplayP3)
    };
    Some((
        [p3.components.0, p3.components.1, p3.components.2],
        p3.alpha,
    ))
}

/// Converts straight sRGB components to Display P3 for the color input's
/// `colorspace=display-p3` serialization.
pub(crate) fn srgb_to_display_p3([r, g, b]: [f32; 3]) -> [f32; 3] {
    let srgb = AbsoluteColor::new(ColorSpace::Srgb, r, g, b, 1.0);
    let p3 = srgb.to_color_space(ColorSpace::DisplayP3);
    [p3.components.0, p3.components.1, p3.components.2]
}

#[cfg(test)]
mod tests {
    use super::parse_exact;

    #[test]
    fn color_mix_is_premultiplied() {
        // Canvas exposes the premultiplied result through `getImageData`; the
        // parser's syntax coverage belongs to WPT `css/css-color/parsing/`.
        let transparent_red = parse_exact("color-mix(in srgb, transparent, red)", None).unwrap();
        assert_eq!(transparent_red.bytes, [255, 0, 0, 128]);
    }
}
