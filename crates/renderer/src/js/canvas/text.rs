//! Canvas text shaping and glyph outlines.
//!
//! Parley resolves the CSS font and shapes the string; skrifa turns the
//! resulting glyph ids into outlines. Both are already part of the document
//! renderer, so canvas text shares its font database and fallback behavior.
//!
//! Spec: <https://html.spec.whatwg.org/multipage/canvas.html#text-styles>

use parley::{
    FontContext, FontFamily, FontStyle, FontWeight, LayoutContext, PositionedLayoutItem,
    StyleProperty,
};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{Path, PathBuilder, Transform};

/// One shaped string, in glyph-local coordinates (baseline at y = 0).
pub(crate) struct ShapedText {
    pub(crate) path: Option<Path>,
    pub(crate) width: f32,
    pub(crate) ascent: f32,
    pub(crate) descent: f32,
    /// Horizontal squeeze applied by `fillText`'s `maxWidth`.
    pub(crate) scale_x: f32,
}

/// Shapes `text` with the canvas `font` shorthand. A `maxWidth` squeezes the
/// result horizontally rather than wrapping it
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-filltext>).
pub(crate) fn shape(
    text: &str,
    font: &str,
    max_width: Option<f32>,
    font_ctx: &mut FontContext,
) -> ShapedText {
    let parsed = parse_font(font);
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
    let mut layout = builder.build(text);
    layout.break_all_lines(None);
    let metrics = layout.root_span_metrics();
    let (ascent, descent) = metrics.map_or((parsed.size, parsed.size * 0.25), |metrics| {
        (metrics.ascent, metrics.descent)
    });
    let width = layout.width();
    let scale_x = max_width
        .filter(|max| *max > 0.0 && width > *max)
        .map_or(1.0, |max| max / width);
    let path = glyph_path(&layout, scale_x);
    ShapedText {
        path,
        width,
        ascent,
        descent,
        scale_x,
    }
}

fn glyph_path(layout: &parley::Layout<[u8; 4]>, scale_x: f32) -> Option<Path> {
    let mut builder = PathBuilder::new();
    let mut placed_any = false;
    for line in layout.lines() {
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
            let run_scale = Transform::from_scale(scale_x, 1.0);
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
                let placement = Transform::from_translate(glyph.x, glyph.y).pre_concat(run_scale);
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
struct ParsedFont {
    size: f32,
    weight: f32,
    italic: bool,
    family: Option<String>,
}

fn parse_font(font: &str) -> ParsedFont {
    let mut size = 10.0;
    let mut weight = 400.0;
    let mut italic = false;
    let tokens: Vec<&str> = font.split_whitespace().collect();
    let mut family_start = None;
    for (index, token) in tokens.iter().enumerate() {
        let lower = token.to_ascii_lowercase();
        if lower == "italic" || lower == "oblique" {
            italic = true;
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
