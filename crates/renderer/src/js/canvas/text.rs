//! Canvas text shaping and glyph outlines.
//!
//! Parley resolves the CSS font and shapes the string; skrifa turns the
//! resulting glyph ids into outlines. Both are already part of the document
//! renderer, so canvas text shares its font database and fallback behavior.
//!
//! Spec: <https://html.spec.whatwg.org/multipage/canvas.html#text-styles>

use parley::{
    FontContext, FontFamily, FontStyle, FontWeight, Language, LayoutContext, PositionedLayoutItem,
    StyleProperty,
};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{Path, PathBuilder, Transform};

/// One shaped grapheme cluster in text space.
pub(crate) struct TextCluster {
    pub(crate) byte_start: usize,
    pub(crate) byte_end: usize,
    pub(crate) utf16_start: usize,
    pub(crate) utf16_end: usize,
    pub(crate) offset: f32,
    pub(crate) advance: f32,
}

/// One shaped string, in glyph-local coordinates (baseline at y = 0).
pub(crate) struct ShapedText {
    pub(crate) path: Option<Path>,
    pub(crate) width: f32,
    pub(crate) ascent: f32,
    pub(crate) descent: f32,
    /// Horizontal squeeze applied by `fillText`'s `maxWidth`.
    pub(crate) scale_x: f32,
    /// Shaped grapheme clusters, in visual order.
    pub(crate) clusters: Vec<TextCluster>,
    /// Glyph ink bounds relative to the alignment point and baseline
    /// (<https://html.spec.whatwg.org/multipage/canvas.html#textmetrics>).
    pub(crate) ink_min_x: f32,
    pub(crate) ink_max_x: f32,
    pub(crate) actual_ascent: f32,
    pub(crate) actual_descent: f32,
    /// Font-level metrics in css pixels, for `TextMetrics`.
    pub(crate) em_ascent: f32,
    pub(crate) em_descent: f32,
    pub(crate) font_ascent: f32,
    pub(crate) font_descent: f32,
    pub(crate) hanging_baseline: f32,
    pub(crate) ideographic_baseline: f32,
}

/// Shapes `text` with the canvas `font` shorthand. A `maxWidth` squeezes the
/// result horizontally rather than wrapping it
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-filltext>).
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "shaping consumes the full canvas text state in one pass"
)]
pub(crate) fn shape(
    text: &str,
    font: &str,
    max_width: Option<f32>,
    letter_spacing: f32,
    word_spacing: f32,
    language: Option<&str>,
    kerning: &str,
    font_ctx: &mut FontContext,
) -> ShapedText {
    // Canvas text preparation replaces ASCII whitespace with U+0020 and
    // shapes one line; newlines are not mandatory breaks
    // (<https://html.spec.whatwg.org/multipage/canvas.html#text-preparation-algorithm>).
    let text: String = text
        .chars()
        .map(|ch| {
            if matches!(ch, '\n' | '\r' | '\t' | '\x0c') {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let parsed = parse_font(font);
    // Canvas replaces every collapsible whitespace with U+0020 and does not
    // collapse runs (<https://html.spec.whatwg.org/multipage/canvas.html#text-preparation-algorithm>).
    // U+0000 has zero advance.
    let prepared: String = text
        .chars()
        .filter(|ch| *ch != '\0')
        .map(|ch| match ch {
            '\t' | '\n' | '\u{c}' | '\r' => ' ',
            _ => ch,
        })
        .collect();
    if prepared.is_empty() {
        return ShapedText {
            path: None,
            width: 0.0,
            ascent: parsed.size,
            descent: parsed.size * 0.25,
            scale_x: 1.0,
            clusters: Vec::new(),
            ink_min_x: 0.0,
            ink_max_x: 0.0,
            actual_ascent: 0.0,
            actual_descent: 0.0,
            em_ascent: parsed.size,
            em_descent: 0.0,
            font_ascent: parsed.size,
            font_descent: 0.0,
            hanging_baseline: parsed.size * 0.5,
            ideographic_baseline: parsed.size * 0.125,
        };
    }
    let text = prepared.as_str();
    let mut layout_ctx = LayoutContext::<[u8; 4]>::new();
    let mut builder = layout_ctx.ranged_builder(font_ctx, text, 1.0, true);
    builder.push_default(StyleProperty::FontSize(parsed.size));
    if let Some(family) = parsed.family {
        builder.push_default(StyleProperty::FontFamily(FontFamily::Source(family.into())));
    }
    if parsed.weight >= 600.0 {
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(parsed.weight)));
    }
    if parsed.italic {
        builder.push_default(StyleProperty::FontStyle(FontStyle::Italic));
    }
    if letter_spacing != 0.0 {
        builder.push_default(StyleProperty::LetterSpacing(letter_spacing));
    }
    if word_spacing != 0.0 {
        builder.push_default(StyleProperty::WordSpacing(word_spacing));
    }
    if let Some(language) = language.and_then(|language| Language::parse(language).ok()) {
        builder.push_default(StyleProperty::Locale(Some(language)));
    }
    // One FontFeatures range: a second push would overwrite the first.
    let mut features = Vec::new();
    if parsed.small_caps {
        // `parse_css_list` requires quoted tags: `"smcp" on`.
        features.push("\"smcp\" 1");
    }
    if kerning == "none" {
        features.push("\"kern\" 0");
    }
    if !features.is_empty() {
        builder.push_default(StyleProperty::FontFeatures(parley::FontFeatures::from(
            features.join(", ").as_str(),
        )));
    }
    let mut layout = builder.build(text);
    layout.break_all_lines(None);
    let metrics = layout.root_span_metrics();
    let (ascent, descent) = metrics.map_or((parsed.size, parsed.size * 0.25), |metrics| {
        (metrics.ascent, metrics.descent)
    });
    // `width()` excludes trailing whitespace; canvas measures it.
    let width = layout.full_width();
    let scale_x = max_width
        .filter(|max| *max > 0.0 && width > *max)
        .map_or(1.0, |max| max / width);
    let utf16_offsets = cluster_utf16_offsets(text);
    let mut clusters = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            for cluster in run.run().clusters() {
                let range = cluster.text_range();
                clusters.push(TextCluster {
                    byte_start: range.start,
                    byte_end: range.end,
                    utf16_start: utf16_offsets[range.start],
                    utf16_end: utf16_offsets[range.end],
                    offset: cluster.visual_offset().unwrap_or(0.0),
                    advance: cluster.advance(),
                });
            }
        }
    }
    let font_metrics = layout_font_metrics(&layout, parsed.size);
    // The glyph path stays in text space; the drawing transform applies the
    // maxWidth squeeze. Metrics need their own squeezed copy.
    let path = glyph_path(&layout);
    let (ink_min_x, ink_max_x, actual_ascent, actual_descent) = path
        .as_ref()
        .and_then(|path| {
            path.clone().transform(
                Transform::from_scale(scale_x, 1.0)
                    .pre_concat(Transform::from_translate(0.0, -ascent)),
            )
        })
        .map(|path| path.bounds())
        .map_or((0.0, 0.0, 0.0, 0.0), |bounds| {
            (
                bounds.left(),
                bounds.right(),
                (-bounds.top()).max(0.0),
                bounds.bottom().max(0.0),
            )
        });
    ShapedText {
        path,
        width,
        ascent,
        descent,
        scale_x,
        clusters,
        ink_min_x,
        ink_max_x,
        actual_ascent,
        actual_descent,
        em_ascent: font_metrics.em_ascent,
        em_descent: font_metrics.em_descent,
        font_ascent: font_metrics.font_ascent,
        font_descent: font_metrics.font_descent,
        hanging_baseline: font_metrics.hanging,
        ideographic_baseline: font_metrics.ideographic,
    }
}

/// Maps every byte boundary in `text` to its UTF-16 offset.
fn cluster_utf16_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0; text.len() + 1];
    let mut utf16 = 0;
    for (byte, ch) in text.char_indices() {
        for slot in &mut offsets[byte..byte + ch.len_utf8()] {
            *slot = utf16;
        }
        utf16 += ch.len_utf16();
    }
    offsets[text.len()] = utf16;
    offsets
}

struct FontMetrics {
    em_ascent: f32,
    em_descent: f32,
    font_ascent: f32,
    font_descent: f32,
    hanging: f32,
    ideographic: f32,
}

/// Canvas separates em-height metrics (win ascent/descent) from font bounding
/// box metrics (hhea/typo ascent/descent); both are scaled by the font size
/// (<https://html.spec.whatwg.org/multipage/canvas.html#textmetrics>).
fn layout_font_metrics(layout: &parley::Layout<[u8; 4]>, size: f32) -> FontMetrics {
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            let run = run.run();
            let font = run.font();
            let Ok(font_ref) = FontRef::from_index(font.data.as_ref(), font.index) else {
                continue;
            };
            let metrics = font_ref.metrics(Size::new(run.font_size()), LocationRef::default());
            // The em square is the font size. A font whose typo metrics do
            // not add up to the em size (the CanvasTest-ascent256 fixture is
            // 256/-256 over 1024 units) is scaled to fill it
            // (<https://html.spec.whatwg.org/multipage/canvas.html#textmetrics>).
            let raw_ascent = metrics.ascent.max(0.0);
            let raw_descent = (-metrics.descent).max(0.0);
            let em_sum = raw_ascent + raw_descent;
            let em_scale = if em_sum > 0.0 {
                run.font_size() / em_sum
            } else {
                1.0
            };
            return FontMetrics {
                em_ascent: raw_ascent * em_scale,
                em_descent: raw_descent * em_scale,
                font_ascent: raw_ascent,
                font_descent: raw_descent,
                hanging: size * 0.5,
                ideographic: size * 0.125,
            };
        }
    }
    FontMetrics {
        em_ascent: size,
        em_descent: 0.0,
        font_ascent: size,
        font_descent: 0.0,
        hanging: size * 0.5,
        ideographic: size * 0.125,
    }
}

fn glyph_path(layout: &parley::Layout<[u8; 4]>) -> Option<Path> {
    let mut builder = PathBuilder::new();
    let mut placed_any = false;
    let root_ascent = layout.root_span_metrics().map_or(0.0, |metrics| metrics.ascent);
    for line in layout.lines() {
        // Parley places glyphs on the CSS line box; canvas text baselines are
        // defined by the font's em metrics, so drop the half-leading.
        let leading = line.metrics().baseline - root_ascent;
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            let shape_run = run.run();
            let font = shape_run.font();
            let Ok(font_ref) = FontRef::from_index(font.data.as_ref(), font.index) else {
                continue;
            };
            let outlines = font_ref.outline_glyphs();
            for glyph in run.positioned_glyphs() {
                let Some(outline) = outlines.get(GlyphId::new(glyph.id)) else {
                    continue;
                };
                let settings = DrawSettings::unhinted(
                    Size::new(shape_run.font_size()),
                    LocationRef::default(),
                );
                let mut pen = GlyphPen::new();
                if outline.draw(settings, &mut pen).is_err() {
                    continue;
                }
                let Some(path) = pen.finish() else {
                    continue;
                };
                let placement = Transform::from_translate(glyph.x, glyph.y - leading);
                let Some(path) = path.transform(placement) else {
                    continue;
                };
                builder.push_path(&path);
                placed_any = true;
            }
        }
    }
    placed_any.then(|| builder.finish()).flatten()
}

/// CSS `font` shorthand: style/weight flags, a `<length>` in px, then the
/// family list (which parley resolves from its default stack)
/// (<https://drafts.csswg.org/css-fonts/#font-prop>).
/// Resolves a canvas length (already normalized by the setter) to CSS pixels
/// against the current font size.
/// (<https://drafts.csswg.org/css-values-4/#lengths>)
pub(crate) fn length_px(value: &str, font_size: f32) -> f32 {
    let split = value
        .find(|ch: char| !(ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+')))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number = number.parse::<f32>().unwrap_or(0.0);
    let scale = match unit.trim() {
        "em" | "ic" => font_size,
        "ex" | "ch" => font_size * 0.5,
        "rem" | "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "pt" => 96.0 / 72.0,

        "q" => 96.0 / 101.6,
        "%" => font_size / 100.0,
        _ => 1.0,
    };
    number * scale
}

/// Canonicalizes the CSS `font` shorthand for `ctx.font`. Relative sizes
/// resolve against the canvas element's font-size/line-height (10px when the
/// element specifies none), and invalid shorthands return `None` so the
/// previous font stays
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-font>).
pub(crate) fn canonical_font(value: &str, base_size: f32, line_height: f32) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if matches!(
        value.to_ascii_lowercase().as_str(),
        "inherit" | "initial" | "unset" | "revert" | "revert-layer" | "default"
    ) || value.contains(['{', '}', ';', '('])
    {
        return None;
    }
    if matches!(
        value,
        "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar"
    ) {
        return Some("10px sans-serif".to_owned());
    }

    let mut style = None;
    let mut variant = None;
    let mut weight = None;
    let mut stretch = None;
    let mut size = None;
    let mut family = None;
    let tokens: Vec<&str> = value.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        if let Some(px) = length_from_token(token, base_size, line_height) {
            size = Some(px);
            family = Some(tokens[index + 1..].join(" "));
            break;
        }
        let lower = token.to_ascii_lowercase();
        match lower.as_str() {
            "italic" | "oblique" => style = Some("italic"),
            "small-caps" => variant = Some("small-caps"),
            "bold" | "bolder" => weight = Some(700.0),
            "lighter" => weight = Some(300.0),
            "normal" => {}
            keyword if is_stretch(keyword) => stretch = Some(keyword.to_owned()),
            number if number.parse::<f32>().is_ok() => {
                let value = number.parse::<f32>().ok()?;
                weight = Some(value);
            }
            _ => return None,
        }
    }
    let size = size?;
    let family = family?;
    if family.is_empty() {
        return None;
    }
    let families = canonical_families(&family)?;
    let mut parts = Vec::new();
    if let Some(style) = style {
        parts.push(style.to_owned());
    }
    if let Some(variant) = variant {
        parts.push(variant.to_owned());
    }
    if let Some(stretch) = stretch.filter(|stretch| stretch != "normal") {
        parts.push(stretch);
    }
    if let Some(weight) = weight.filter(|weight| (weight - 400.0).abs() > f32::EPSILON) {
        parts.push(format_number(weight));
    }
    parts.push(format!("{}px", format_number(size)));
    parts.push(families);
    Some(parts.join(" "))
}

fn length_from_token(token: &str, base_size: f32, line_height: f32) -> Option<f32> {
    let token = token.split('/').next().unwrap_or(token);
    let split = token.find(|ch: char| !(ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+')))?;
    let (number, unit) = token.split_at(split);
    let number = number.parse::<f32>().ok()?;
    if !number.is_finite() {
        return None;
    }
    let scale = match unit.to_ascii_lowercase().as_str() {
        "px" => 1.0,
        "%" => base_size / 100.0,
        "em" | "ic" => base_size,
        "rem" | "pc" => 16.0,
        "ex" | "ch" => base_size * 0.5,
        "lh" => line_height,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "pt" => 96.0 / 72.0,
        "q" => 96.0 / 101.6,
        _ => return None,
    };
    Some(number * scale)
}

fn is_stretch(value: &str) -> bool {
    matches!(
        value,
        "ultra-condensed"
            | "extra-condensed"
            | "condensed"
            | "semi-condensed"
            | "normal"
            | "semi-expanded"
            | "expanded"
            | "extra-expanded"
            | "ultra-expanded"
    )
}

fn canonical_families(value: &str) -> Option<String> {
    let mut families = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => {
                current.push(ch);
                escaped = true;
            }
            '"' if quoted => {
                quoted = false;
                current.push(ch);
            }
            '"' => {
                quoted = true;
                current.push(ch);
            }
            ',' if !quoted => {
                families.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    families.push(current);
    let mut output = Vec::new();
    for family in families {
        let family = family.trim();
        if family.is_empty() || family.contains(['{', '}', ';', '(', ')']) {
            return None;
        }
        if matches!(
            family.to_ascii_lowercase().as_str(),
            "inherit" | "initial" | "unset" | "revert" | "revert-layer" | "default"
        ) {
            return None;
        }
        let lower = family.to_ascii_lowercase();
        if is_generic_family(&lower) {
            output.push(lower);
        } else if family.starts_with('"') {
            output.push(family.to_owned());
        } else if family.contains(' ') {
            output.push(format!("\"{family}\""));
        } else {
            output.push(family.to_owned());
        }
    }
    Some(output.join(", "))
}

fn is_generic_family(value: &str) -> bool {
    matches!(
        value,
        "serif"
            | "sans-serif"
            | "monospace"
            | "cursive"
            | "fantasy"
            | "system-ui"
            | "ui-serif"
            | "ui-sans-serif"
            | "ui-monospace"
            | "ui-rounded"
            | "math"
            | "emoji"
            | "fangsong"
    )
}

fn format_number(value: f32) -> String {
    if (value.fract()).abs() < 0.0001 {
        format!("{}", value.round())
    } else {
        format!("{value}")
    }
}

/// The px size from a `font` shorthand, for resolving font-relative spacing.
pub(crate) fn font_size(font: &str) -> f32 {
    parse_font(font).size
}

struct ParsedFont {
    size: f32,
    weight: f32,
    italic: bool,
    small_caps: bool,
    family: Option<String>,
}

fn parse_font(font: &str) -> ParsedFont {
    let mut size = 10.0;
    let mut weight = 400.0;
    let mut italic = false;
    let mut small_caps = false;
    let tokens: Vec<&str> = font.split_whitespace().collect();
    let mut family_start = None;
    for (index, token) in tokens.iter().enumerate() {
        let lower = token.to_ascii_lowercase();
        if lower == "italic" || lower == "oblique" {
            italic = true;
        } else if lower == "small-caps" {
            small_caps = true;
        } else if lower == "bold" || lower == "bolder" {
            weight = 700.0;
        } else if let Some(px) = lower.strip_suffix("px") {
            // `12px/1.5` carries a line height after the size.
            let px = px.split('/').next().unwrap_or(px);
            if let Ok(value) = px.parse::<f32>() {
                size = value;
                family_start = Some(index + 1);
                break;
            }
        } else if let Ok(value) = lower.parse::<f32>()
            && (1.0..=1000.0).contains(&value)
            && lower.len() <= 3
        {
            weight = value;
        }
    }
    let family = family_start
        .filter(|start| *start < tokens.len())
        .map(|start| tokens[start..].join(" "));
    ParsedFont {
        size: size.max(0.0),
        weight,
        italic,
        small_caps,
        family,
    }
}

/// Collects one glyph outline, flipping font y-up coordinates to canvas
/// y-down around the baseline.
struct GlyphPen {
    builder: PathBuilder,
}

impl GlyphPen {
    fn new() -> Self {
        Self {
            builder: PathBuilder::new(),
        }
    }

    fn finish(self) -> Option<Path> {
        self.builder.finish()
    }
}

impl OutlinePen for GlyphPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(x, -y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(x, -y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.builder.quad_to(cx0, -cy0, x, -y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.builder.cubic_to(cx0, -cy0, cx1, -cy1, x, -y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}
