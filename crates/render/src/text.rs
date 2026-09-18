//! Text shaping and line breaking over Parley.
//!
//! One inline formatting context becomes one Parley layout: text segments
//! with their styles, atomic boxes as inline boxes, one break pass at the
//! leaf width. Parley owns shaping (ligatures, kerning, bidi), line breaking
//! (UAX#14), and alignment; this module translates our styles in and painted
//! runs back out.
//!
//! Intrinsic widths (`measure`, `min_content_width`) read `skrifa` advance
//! widths directly: they answer per-word questions Parley is not asked.
//! Paint reads glyph IDs and positions from here and rasterizes through
//! `skrifa` outlines, so shaped text paints exactly as positioned.

use parley::layout::PositionedLayoutItem;
use parley::style::{
    FontFamily, FontWeight, LineHeight as ParleyLineHeight, TextStyle, WhiteSpaceCollapse,
};
use parley::{Alignment, AlignmentOptions};

use crate::color::Color;
use crate::font::{Fonts, Weight};
use crate::style::{LineHeight, TextAlign, TextDecoration};

/// The font properties for intrinsic measurement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FontStyle {
    /// `font-size` in CSS pixels.
    pub size: f32,
    /// Resolved `font-weight`.
    pub weight: Weight,
}

/// The style of one text segment pushed to Parley.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SegmentStyle {
    /// Font size in pixels.
    pub size: f32,
    /// Font weight.
    pub weight: Weight,
    /// Text color.
    pub color: Color,
    /// Decoration line.
    pub decoration: TextDecoration,
    /// Extra advance per character.
    pub letter_spacing: f32,
    /// Line height.
    pub line_height: LineHeight,
    /// Whether the text paints; `visibility: hidden` text still shapes and
    /// occupies space but must not be drawn.
    pub visible: bool,
}

impl SegmentStyle {
    /// Builds a segment style from a computed style.
    pub(crate) fn from_style(style: &crate::style::Style) -> Self {
        Self {
            size: style.font_size,
            weight: style.font_weight,
            color: style.color,
            decoration: style.text_decoration,
            letter_spacing: style.letter_spacing,
            line_height: style.line_height,
            visible: style.visibility == crate::style::Visibility::Visible,
        }
    }
}

/// One input token in document order.
pub(crate) enum Segment<'a> {
    /// Text with its style.
    Text(&'a str, SegmentStyle),
    /// An atomic box; the index addresses the caller's measurements.
    Atomic(usize),
}

/// One shaped glyph with leaf-local position.
#[derive(Clone, Debug)]
pub(crate) struct PlacedGlyph {
    /// Glyph ID in the run's font.
    pub id: u32,
    /// Left edge reference position.
    pub x: f32,
    /// Baseline-relative vertical position.
    pub y: f32,
}

/// One painted glyph run: glyphs sharing size, weight, color, and baseline.
#[derive(Clone, Debug)]
pub(crate) struct PlacedRun {
    /// The glyphs in visual order.
    pub glyphs: Vec<PlacedGlyph>,
    /// Font size in pixels.
    pub size: f32,
    /// Font weight.
    pub weight: Weight,
    /// Text color.
    pub color: Color,
    /// Decoration line.
    pub decoration: TextDecoration,
    /// Absolute baseline for decoration lines.
    pub baseline: f32,
    /// Left edge of the run extent.
    pub x: f32,
    /// Run extent width.
    pub width: f32,
}

/// One placed atomic box, addressing the caller's measurements.
#[derive(Clone, Debug)]
pub(crate) struct PlacedAtomic {
    /// Index into the caller's atomic measurements.
    pub index: usize,
    /// Left edge, leaf-local.
    pub x: f32,
    /// Top edge, leaf-local.
    pub y: f32,
}

/// One shaped line.
#[derive(Clone, Debug)]
pub(crate) struct PlacedLine {
    /// Line box height.
    pub height: f32,
    /// Glyph runs in visual order.
    pub runs: Vec<PlacedRun>,
    /// Atomic boxes in visual order.
    pub atomics: Vec<PlacedAtomic>,
}

impl Fonts {
    /// The advance width of a string with no shaping or kerning, from the
    /// face's own `hmtx` table.
    ///
    /// Intrinsic widths only; painted text is shaped by Parley, which applies
    /// kerning and ligatures itself.
    pub(crate) fn measure(&self, text: &str, style: FontStyle) -> f32 {
        use skrifa::MetadataProvider as _;
        use skrifa::instance::{LocationRef, NormalizedCoord, Size};
        let face = self.outline_face(style.weight);
        let charmap = face.charmap();
        let coords: &[NormalizedCoord] = &[];
        let metrics = skrifa::metrics::GlyphMetrics::new(
            &face,
            Size::new(style.size),
            LocationRef::from(coords),
        );
        text.chars()
            .filter_map(|ch| charmap.map(ch))
            .filter_map(|glyph| metrics.advance_width(glyph))
            .sum()
    }
}

/// Shapes `segments` into lines at `width` (`None` disables wrapping).
///
/// Atomic sizes come pre-measured from the caller; each [`Segment::Atomic`]
/// carries the index into `atomics`. `preserve_space` selects Parley's
/// whitespace mode for the whole leaf; callers pre-collapse collapsing runs
/// themselves so mixed modes stay exact.
pub(crate) fn shape_lines(
    fonts: &Fonts,
    segments: &[Segment<'_>],
    atomics: &[(f32, f32)],
    width: Option<f32>,
    align: TextAlign,
    preserve_space: bool,
) -> Vec<PlacedLine> {
    let mut shaped = fonts.parley.borrow_mut();
    let shaped = &mut *shaped;
    let root = root_style();
    let mut builder =
        shaped
            .layout_context
            .tree_builder(&mut shaped.font_context, 1.0, true, &root);
    builder.set_white_space_mode(if preserve_space {
        WhiteSpaceCollapse::Preserve
    } else {
        WhiteSpaceCollapse::Collapse
    });
    let mut bytes = 0usize;
    for segment in segments {
        match segment {
            Segment::Text(text, style) => {
                builder.push_style_span(text_style(style));
                builder.push_text(text);
                bytes += text.len();
            }
            Segment::Atomic(index) => {
                let (width, height) = atomics.get(*index).copied().unwrap_or((0.0, 0.0));
                builder.push_inline_box(parley::InlineBox {
                    id: *index as u64,
                    kind: parley::InlineBoxKind::InFlow,
                    index: bytes,
                    width,
                    height,
                });
            }
        }
    }
    let (mut layout, _) = builder.build();
    match width {
        Some(width) => layout.break_all_lines(Some(width.max(0.0))),
        None => layout.break_all_lines(None),
    }
    layout.align(
        match align {
            TextAlign::Left => Alignment::Start,
            TextAlign::Right => Alignment::End,
            TextAlign::Center => Alignment::Center,
        },
        AlignmentOptions::default(),
    );
    let mut lines = Vec::new();
    for line in layout.lines() {
        let metrics = line.metrics();
        let mut runs = Vec::new();
        let mut placed_atomics = Vec::new();
        for item in line.items() {
            match item {
                PositionedLayoutItem::GlyphRun(run) => {
                    if let Some(placed) = place_run(&run) {
                        runs.push(placed);
                    }
                }
                PositionedLayoutItem::InlineBox(atom) => {
                    let index = usize::try_from(atom.id).unwrap_or(usize::MAX);
                    if index < atomics.len() {
                        placed_atomics.push(PlacedAtomic {
                            index,
                            x: atom.x,
                            y: atom.y,
                        });
                    }
                }
            }
        }
        lines.push(PlacedLine {
            height: metrics.line_height,
            runs,
            atomics: placed_atomics,
        });
    }
    lines
}

/// The root style: our single family at a neutral size; spans cover all
/// pushed text.
fn root_style() -> TextStyle<'static, 'static, [u8; 4]> {
    TextStyle {
        font_family: FontFamily::named("Liberation Sans"),
        ..Default::default()
    }
}

/// Translates one segment style to Parley's.
fn text_style(style: &SegmentStyle) -> TextStyle<'static, 'static, [u8; 4]> {
    TextStyle {
        font_family: FontFamily::named("Liberation Sans"),
        font_size: style.size,
        font_weight: match style.weight {
            Weight::Normal => FontWeight::NORMAL,
            Weight::Bold => FontWeight::BOLD,
        },
        // `visibility: hidden` rides the brush alpha: the glyphs keep their
        // metrics and positions, and every painter path skips alpha 0.
        brush: if style.visible {
            [style.color.r, style.color.g, style.color.b, style.color.a]
        } else {
            [style.color.r, style.color.g, style.color.b, 0]
        },
        has_underline: style.decoration == TextDecoration::Underline,
        has_strikethrough: style.decoration == TextDecoration::LineThrough,
        letter_spacing: style.letter_spacing,
        line_height: match style.line_height {
            LineHeight::Normal => ParleyLineHeight::MetricsRelative(1.0),
            LineHeight::Number(factor) => ParleyLineHeight::FontSizeRelative(factor),
            LineHeight::Px(px) => ParleyLineHeight::Absolute(px),
        },
        ..Default::default()
    }
}

/// Converts one positioned glyph run, or `None` when it holds no glyphs.
fn place_run(run: &parley::layout::GlyphRun<'_, [u8; 4]>) -> Option<PlacedRun> {
    let inner = run.run();
    let run_style = run.style();
    let color = brush_color(run_style.brush);
    let decoration = if run_style.underline.is_some() {
        TextDecoration::Underline
    } else if run_style.strikethrough.is_some() {
        TextDecoration::LineThrough
    } else {
        TextDecoration::None
    };
    let size = inner.font_size();
    let weight = if inner.font_attrs().weight >= fontique::FontWeight::BOLD {
        Weight::Bold
    } else {
        Weight::Normal
    };
    let mut glyphs = Vec::new();
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    // Positions and baselines are layout-relative: Parley accumulates line
    // tops itself, so no per-line offset here.
    for glyph in run.positioned_glyphs() {
        min_x = min_x.min(glyph.x);
        max_x = max_x.max(glyph.x + glyph.advance);
        glyphs.push(PlacedGlyph {
            id: glyph.id,
            x: glyph.x,
            y: glyph.y,
        });
    }
    if glyphs.is_empty() {
        return None;
    }
    Some(PlacedRun {
        glyphs,
        size,
        weight,
        color,
        decoration,
        baseline: run.baseline(),
        x: min_x,
        width: (max_x - min_x).max(0.0),
    })
}

/// Reads our color back from a Parley brush.
fn brush_color(brush: [u8; 4]) -> Color {
    Color {
        r: brush[0],
        g: brush[1],
        b: brush[2],
        a: brush[3],
    }
}
