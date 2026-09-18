//! Box layout: block containers, inline formatting, and atomic inline boxes.
//!
//! The formatting model follows CSS 2.1's visual formatting model
//! (<https://drafts.csswg.org/css2/#visuren>) and CSS Text for whitespace and
//! line breaking (<https://drafts.csswg.org/css-text-3/#text-formatting>).
//! Flex containers are dispatched to `flex.rs`.
//!
//! Coordinates are CSS pixels with the origin at the viewport's top-left;
//! every `LayoutBox.rect` is the border box in absolute coordinates. Paint
//! walks the tree and blits text runs in order.

use crate::color::Color;
use crate::font::{Fonts, Weight};
use crate::geometry::{Edges, Rect};
use crate::style::{BoxSizing, Dimension, Style, TextAlign, TextDecoration, WhiteSpace};
use crate::text::FontStyle;
use crate::tree::{BoxKind, BoxNode};

/// One line of text ready to paint.
#[derive(Clone, Debug)]
pub(crate) struct TextRun {
    /// The run text.
    pub(crate) text: String,
    /// Left edge.
    pub(crate) x: f32,
    /// Alphabetic baseline.
    pub(crate) baseline: f32,
    /// Advance width.
    pub(crate) width: f32,
    /// Font size in pixels.
    pub(crate) font_size: f32,
    /// Font weight.
    pub(crate) weight: Weight,
    /// Text color.
    pub(crate) color: Color,
    /// Decoration line.
    pub(crate) decoration: TextDecoration,
}

/// One painted child in tree order.
pub(crate) enum PaintItem {
    /// A nested box.
    Box(Box<LayoutBox>),
    /// A run of text.
    Text(TextRun),
}

/// A laid-out box.
pub(crate) struct LayoutBox {
    /// Computed style of the originating element (or the anonymous box).
    pub(crate) style: Style,
    /// Border box in absolute coordinates.
    pub(crate) rect: Rect,
    /// Resolved padding in pixels.
    pub(crate) padding: Edges<f32>,
    /// Children and text runs in paint order.
    pub(crate) items: Vec<PaintItem>,
}

impl LayoutBox {
    /// The padding box: the border box minus its borders.
    pub(crate) fn padding_box(&self) -> Rect {
        let border = self.style.border;
        Rect::new(
            self.rect.x + border.left.width,
            self.rect.y + border.top.width,
            self.rect.width - border.left.width - border.right.width,
            self.rect.height - border.top.width - border.bottom.width,
        )
    }

    /// The content box: the padding box minus its padding.
    pub(crate) fn content_box(&self) -> Rect {
        let padding = self.padding;
        let border = self.style.border;
        Rect::new(
            self.rect.x + border.left.width + padding.left,
            self.rect.y + border.top.width + padding.top,
            self.rect.width - border.left.width - border.right.width - padding.left - padding.right,
            self.rect.height - border.top.width - border.bottom.width - padding.top - padding.bottom,
        )
    }
}

/// Per-layout context: fonts and the root font size.
pub(crate) struct Ctx<'a> {
    /// Embedded faces.
    pub(crate) fonts: &'a Fonts,
    /// Root element font size for `rem`.
    pub(crate) root_font_size: f32,
}


/// The result of laying out one run of inline-level children.
pub(crate) struct InlineResult {
    /// Total height of the line boxes.
    pub(crate) height: f32,
    /// Text runs and atomic boxes in paint order.
    pub(crate) items: Vec<PaintItem>,
}

/// One inline-level item collected before line breaking.
#[expect(
    clippy::large_enum_variant,
    reason = "styles stay inline cloned data; boxing every word would allocate per token"
)]
enum InlineItem<'a> {
    /// A word.
    Word { text: String, style: Style },
    /// A space between items.
    Space { width: f32 },
    /// A forced line break.
    Break,
    /// An atomic inline-level box.
    Atomic {
        node: &'a BoxNode,
        measurement: Atomic,
    },
}

/// Measured atomic box.
#[derive(Clone, Copy, Debug)]
struct Atomic {
    /// Forced content width used to lay the box out.
    content_width: f32,
    /// Border-box height.
    height: f32,
    /// Outer width including margins.
    outer_width: f32,
}

/// Lays out `run` into line boxes starting at `(x, y)`.
pub(crate) fn layout_inline_run(
    run: &[BoxNode],
    ctx: &Ctx<'_>,
    x: f32,
    y: f32,
    available: f32,
    text_align: TextAlign,
) -> InlineResult {
    let mut items: Vec<InlineItem<'_>> = Vec::new();
    for node in run {
        collect_inline(node, ctx, &mut items);
    }

    let mut painted: Vec<PaintItem> = Vec::new();
    let mut line = Line::default();
    let mut cursor_y = y;
    let line_box = LineBox {
        x,
        available,
        text_align,
    };

    for item in items {
        match item {
            InlineItem::Word { text, style } => {
                let font = FontStyle {
                    size: style.font_size,
                    weight: style.font_weight,
                };
                let width = measure(&text, font, style.letter_spacing, ctx.fonts);
                let wrap = matches!(style.white_space, WhiteSpace::Normal | WhiteSpace::PreWrap);
                if wrap && !line.is_empty() && line.width + width > available {
                    cursor_y += flush_line(&mut line, &mut painted, &line_box, cursor_y, ctx);
                }
                line.width += width;
                line.update_metrics(&style, ctx);
                line.items.push(LineItem::Word { text, style, width });
            }
            InlineItem::Space { width } => {
                if !line.is_empty() {
                    line.width += width;
                    line.items.push(LineItem::Space { width });
                }
            }
            InlineItem::Break => {
                cursor_y += flush_line(&mut line, &mut painted, &line_box, cursor_y, ctx);
            }
            InlineItem::Atomic { node, measurement } => {
                if !line.is_empty() && line.width + measurement.outer_width > available {
                    cursor_y += flush_line(&mut line, &mut painted, &line_box, cursor_y, ctx);
                }
                line.width += measurement.outer_width;
                line.ascent = line.ascent.max(measurement.height);
                line.height = line.height.max(measurement.height);
                line.items.push(LineItem::Atomic { node, measurement });
            }
        }
    }
    cursor_y += flush_line(&mut line, &mut painted, &line_box, cursor_y, ctx);

    InlineResult {
        height: cursor_y - y,
        items: painted,
    }
}

/// One item inside a line under construction.
#[expect(
    clippy::large_enum_variant,
    reason = "styles stay inline cloned data; boxing line items would allocate per word"
)]
enum LineItem<'a> {
    /// A word.
    Word {
        text: String,
        style: Style,
        width: f32,
    },
    /// A space between items.
    Space { width: f32 },
    /// An atomic box.
    Atomic {
        node: &'a BoxNode,
        measurement: Atomic,
    },
}

/// A line box under construction.
#[derive(Default)]
struct Line<'a> {
    items: Vec<LineItem<'a>>,
    width: f32,
    ascent: f32,
    descent: f32,
    height: f32,
}

impl Line<'_> {
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn update_metrics(&mut self, style: &Style, ctx: &Ctx<'_>) {
        let font = FontStyle {
            size: style.font_size,
            weight: style.font_weight,
        };
        let metrics = ctx.fonts.line_metrics(font);
        let line_height = style.used_line_height(metrics);
        let half_leading = (line_height - (metrics.ascent + metrics.descent)) / 2.0;
        self.ascent = self.ascent.max(metrics.ascent + half_leading);
        self.descent = self.descent.max(metrics.descent + half_leading);
        self.height = self.height.max(line_height);
    }
}

/// The line box geometry shared by every line of one inline run.
struct LineBox {
    /// Left edge of the content area.
    x: f32,
    /// Available width for one line.
    available: f32,
    /// Horizontal alignment of short lines.
    text_align: TextAlign,
}

/// Places the pending line and returns the height it occupied.
fn flush_line(
    line: &mut Line<'_>,
    painted: &mut Vec<PaintItem>,
    line_box: &LineBox,
    y: f32,
    ctx: &Ctx<'_>,
) -> f32 {
    if line.is_empty() {
        return 0.0;
    }
    // A trailing space does not count toward the line or its alignment.
    // (`pop` in the scrutinee would discard a trailing word, so check first.)
    if matches!(line.items.last(), Some(LineItem::Space { .. }))
        && let Some(LineItem::Space { width }) = line.items.pop()
    {
        line.width -= width;
    }
    if line.items.is_empty() {
        *line = Line::default();
        return 0.0;
    }

    let line_height = line.height.max(line.ascent + line.descent);
    let baseline = y + (line_height - line.ascent - line.descent).max(0.0) / 2.0 + line.ascent;
    let slack = (line_box.available - line.width).max(0.0);
    let align = match line_box.text_align {
        TextAlign::Left => 0.0,
        TextAlign::Right => slack,
        TextAlign::Center => slack / 2.0,
    };
    let mut cursor = line_box.x + align;
    for item in line.items.drain(..) {
        match item {
            LineItem::Word {
                text,
                style,
                width,
            } => {
                painted.push(PaintItem::Text(TextRun {
                    text,
                    x: cursor,
                    baseline,
                    width,
                    font_size: style.font_size,
                    weight: style.font_weight,
                    color: style.color,
                    decoration: style.text_decoration,
                }));
                cursor += width;
            }
            LineItem::Space { width } => cursor += width,
            LineItem::Atomic { node, measurement } => {
                let mut layout =
                    crate::boxes::layout_subtree(node, ctx, measurement.content_width);
                shift_layout(&mut layout, cursor, baseline - measurement.height);
                painted.push(PaintItem::Box(Box::new(layout)));
                cursor += measurement.outer_width;
            }
        }
    }
    line.width = 0.0;
    line.ascent = 0.0;
    line.descent = 0.0;
    line.height = 0.0;
    line_height
}

/// Recursively flattens inline-level boxes into items.
fn collect_inline<'a>(node: &'a BoxNode, ctx: &Ctx<'_>, out: &mut Vec<InlineItem<'a>>) {
    match &node.kind {
        BoxKind::Text(text) => tokenize(text, &node.style, ctx, out),
        BoxKind::Break => out.push(InlineItem::Break),
        BoxKind::Inline => {
            for child in &node.children {
                collect_inline(child, ctx, out);
            }
        }
        BoxKind::InlineBlock | BoxKind::InlineFlex | BoxKind::InlineGrid | BoxKind::Block
        | BoxKind::Flex | BoxKind::Grid | BoxKind::ListItem => {
            let measurement = measure_atomic(node, ctx);
            out.push(InlineItem::Atomic { node, measurement });
        }
    }
}

/// Splits text into words, spaces, and breaks per `white-space`
/// (<https://drafts.csswg.org/css-text-3/#white-space-processing>).
fn tokenize<'a>(text: &str, style: &Style, ctx: &Ctx<'_>, out: &mut Vec<InlineItem<'a>>) {
    let font = FontStyle {
        size: style.font_size,
        weight: style.font_weight,
    };
    let space_width = measure(" ", font, style.letter_spacing, ctx.fonts);
    let collapse = matches!(style.white_space, WhiteSpace::Normal | WhiteSpace::Nowrap);

    let mut word = String::new();
    let mut pending_space = false;

    let flush_word = |word: &mut String, out: &mut Vec<InlineItem<'a>>| {
        if !word.is_empty() {
            out.push(InlineItem::Word {
                text: std::mem::take(word),
                style: style.clone(),
            });
        }
    };

    for ch in text.chars() {
        let is_collapsible = matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}');
        if is_collapsible {
            if ch == '\n' && !collapse {
                flush_word(&mut word, out);
                out.push(InlineItem::Break);
                continue;
            }
            if collapse {
                flush_word(&mut word, out);
                pending_space = true;
            } else {
                flush_word(&mut word, out);
                out.push(InlineItem::Space { width: space_width });
            }
            continue;
        }
        if pending_space {
            out.push(InlineItem::Space { width: space_width });
            pending_space = false;
        }
        // Non-breaking spaces stay inside words and never break.
        word.push(if ch == '\u{a0}' { ' ' } else { ch });
    }
    flush_word(&mut word, out);
}

/// Measures text with optional letter spacing.
fn measure(text: &str, font: FontStyle, letter_spacing: f32, fonts: &Fonts) -> f32 {
    let base = fonts.measure(text, font);
    if letter_spacing == 0.0 {
        base
    } else {
        base + letter_spacing * crate::count(text.chars().count())
    }
}

/// Measures an atomic box at its max-content width, through a nested Taffy
/// tree rooted at the atomic.
fn measure_atomic(node: &BoxNode, ctx: &Ctx<'_>) -> Atomic {
    let preferred = max_content_width(node, ctx);
    let layout = crate::boxes::layout_subtree(node, ctx, preferred);
    let margin = node.style.margin.map(|dimension| match dimension {
        Dimension::Auto => 0.0,
        Dimension::Length(length) => {
            length.resolve(preferred, node.style.font_size, ctx.root_font_size)
        }
    });
    Atomic {
        content_width: layout.content_box().width,
        height: layout.rect.height,
        outer_width: layout.rect.width + margin.left + margin.right,
    }
}

/// Shifts a laid-out subtree by `(dx, dy)`, used to place atomic boxes laid
/// out at the origin onto the line.
pub(crate) fn shift_layout(layout: &mut LayoutBox, dx: f32, dy: f32) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    layout.rect.x += dx;
    layout.rect.y += dy;
    shift_items(&mut layout.items, dx, dy);
}

/// Shifts paint items by `(dx, dy)`.
pub(crate) fn shift_items(items: &mut Vec<PaintItem>, dx: f32, dy: f32) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    for item in items {
        match item {
            PaintItem::Box(child) => shift_layout(child, dx, dy),
            PaintItem::Text(run) => {
                run.x += dx;
                run.baseline += dy;
            }
        }
    }
}

/// Min-content width of one inline run: the longest unbreakable word
/// (<https://drafts.csswg.org/css-sizing-3/#min-content>).
pub(crate) fn min_content_width(run: &[BoxNode], ctx: &Ctx<'_>) -> f32 {
    let mut longest = 0.0_f32;
    for node in run {
        longest = longest.max(min_content_node(node, ctx));
    }
    longest
}

/// Min-content contribution of one inline-level box.
fn min_content_node(node: &BoxNode, ctx: &Ctx<'_>) -> f32 {
    let style = &node.style;
    let font = FontStyle {
        size: style.font_size,
        weight: style.font_weight,
    };
    match &node.kind {
        BoxKind::Text(text) => {
            let mut longest = 0.0_f32;
            let mut current = String::new();
            let mut flush = |current: &mut String| {
                if !current.is_empty() {
                    longest = longest.max(measure(current, font, style.letter_spacing, ctx.fonts));
                    current.clear();
                }
            };
            for ch in text.chars() {
                if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
                    flush(&mut current);
                } else {
                    current.push(ch);
                }
            }
            flush(&mut current);
            longest
        }
        BoxKind::Inline => node
            .children
            .iter()
            .map(|child| min_content_node(child, ctx))
            .sum(),
        BoxKind::Break => 0.0,
        // Atomics contribute their max-content size as a unit.
        BoxKind::InlineBlock | BoxKind::InlineFlex | BoxKind::InlineGrid | BoxKind::Block
        | BoxKind::Flex | BoxKind::Grid | BoxKind::ListItem => max_content_width(node, ctx),
    }
}

/// Padding plus border of a box: the difference between content and border
/// box.
fn outer_extras(node: &BoxNode, ctx: &Ctx<'_>) -> f32 {
    let style = &node.style;
    let padding = style.padding.left.resolve(0.0, style.font_size, ctx.root_font_size)
        + style.padding.right.resolve(0.0, style.font_size, ctx.root_font_size);
    padding + style.border.left.width + style.border.right.width
}

/// Max-content width of a box
/// (<https://drafts.csswg.org/css-sizing-3/#max-content>).
pub(crate) fn max_content_width(node: &BoxNode, ctx: &Ctx<'_>) -> f32 {
    let style = &node.style;
    let extras = outer_extras(node, ctx);
    let content = match &node.kind {
        BoxKind::Text(text) => {
            let font = FontStyle {
                size: style.font_size,
                weight: style.font_weight,
            };
            let mut width = 0.0_f32;
            let mut current = 0.0_f32;
            for ch in text.chars() {
                if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
                    width = width.max(current);
                    current = 0.0;
                    width += measure(" ", font, style.letter_spacing, ctx.fonts);
                } else {
                    current += measure(&ch.to_string(), font, style.letter_spacing, ctx.fonts);
                }
            }
            width.max(current)
        }
        BoxKind::Break => 0.0,
        BoxKind::Inline => node
            .children
            .iter()
            .map(|child| max_content_width(child, ctx))
            .sum(),
        BoxKind::Flex | BoxKind::InlineFlex => {
            let row = matches!(
                style.flex_direction,
                crate::style::FlexDirection::Row
                    | crate::style::FlexDirection::RowReverse
            );
            let widths = node
                .children
                .iter()
                .map(|child| max_content_width(child, ctx));
            if row {
                widths.sum()
            } else {
                widths.fold(0.0, f32::max)
            }
        }
        BoxKind::InlineBlock | BoxKind::InlineGrid => node
            .children
            .iter()
            .map(|child| max_content_width(child, ctx))
            .fold(0.0, f32::max),
        // Grid max-content is approximated like flex rows; Taffy resolves
        // the exact intrinsic sizes during layout.
        BoxKind::Grid => node
            .children
            .iter()
            .map(|child| max_content_width(child, ctx))
            .fold(0.0, f32::max),
        BoxKind::Block | BoxKind::ListItem => node
            .children
            .iter()
            .map(|child| max_content_width(child, ctx))
            .fold(0.0, f32::max),
    };
    match style.width {
        Dimension::Length(length) => {
            let value = length.resolve(0.0, style.font_size, ctx.root_font_size);
            if style.box_sizing == BoxSizing::BorderBox {
                value
            } else {
                value + extras
            }
        }
        Dimension::Auto => content + extras,
    }
}

