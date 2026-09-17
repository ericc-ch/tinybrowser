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
use crate::tree::{BoxKind, BoxNode, is_block_level};

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
    /// Viewport height for percentage heights against the initial containing
    /// block.
    pub(crate) viewport_height: f32,
}

/// The containing block passed down to one layout call.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Containing {
    /// Content width in pixels.
    pub(crate) width: f32,
    /// Content height when definite.
    pub(crate) height: Option<f32>,
    /// Border-box left edge.
    pub(crate) x: f32,
    /// Border-box top edge.
    pub(crate) y: f32,
}

/// Lays out the root wrapper.
pub(crate) fn layout_root(
    root: &BoxNode,
    fonts: &Fonts,
    viewport_width: f32,
    viewport_height: f32,
) -> LayoutBox {
    let ctx = Ctx {
        fonts,
        root_font_size: root.style.font_size,
        viewport_height,
    };
    let containing = Containing {
        width: viewport_width,
        height: Some(viewport_height),
        x: 0.0,
        y: 0.0,
    };
    layout_block(root, &ctx, containing, None)
}

/// Builds a layout box for `b`, positioned at its containing block's origin
/// plus its own margins.
pub(crate) fn layout_block(
    b: &BoxNode,
    ctx: &Ctx<'_>,
    containing: Containing,
    forced_content_width: Option<f32>,
) -> LayoutBox {
    layout_block_sized(b, ctx, containing, forced_content_width, None)
}

/// [`layout_block`] with an optional forced content height, used by flex
/// stretch and grow.
pub(crate) fn layout_block_sized(
    b: &BoxNode,
    ctx: &Ctx<'_>,
    containing: Containing,
    forced_content_width: Option<f32>,
    forced_content_height: Option<f32>,
) -> LayoutBox {
    let style = b.style;
    let font_size = style.font_size;
    let basis = containing.width;
    let root = ctx.root_font_size;

    let BoxMetrics {
        margin,
        padding,
        border,
        extras,
    } = box_metrics(&style, basis, font_size, root);

    let specified = forced_content_width.or(dimension_value(style.width, basis, font_size, root));
    let content_width = if let Some(width) = specified {
        let content = if style.box_sizing == BoxSizing::BorderBox {
            (width - extras).max(0.0)
        } else {
            width
        };
        clamp_dimension(
            content,
            style.min_width,
            style.max_width,
            basis,
            font_size,
            root,
        )
    } else {
        let available = (basis - margin.left - margin.right - extras).max(0.0);
        clamp_dimension(
            available,
            style.min_width,
            style.max_width,
            basis,
            font_size,
            root,
        )
    };
    let (margin_left, _margin_right) = if forced_content_width.is_some() {
        (margin.left, margin.right)
    } else {
        resolve_auto_margins(
            style.margin,
            margin,
            style.width,
            basis,
            content_width + extras,
        )
    };

    let border_x = containing.x + margin_left;
    let border_y = containing.y + margin.top;
    let content_x = border_x + border.left + padding.left;
    let content_y = border_y + border.top + padding.top;

    let specified_height = dimension_value(
        style.height,
        containing.height.unwrap_or(ctx.viewport_height),
        font_size,
        root,
    );
    let content_height_limit = specified_height.map(|height| {
        if style.box_sizing == BoxSizing::BorderBox {
            (height - padding.vertical() - border.vertical()).max(0.0)
        } else {
            height
        }
    });
    let content_height_limit = forced_content_height.or(content_height_limit);

    let (items, cursor_y) = layout_children(
        b,
        ctx,
        content_x,
        content_y,
        content_width,
        content_height_limit.or(containing.height),
        style.text_align,
    );

    let content_height = match content_height_limit {
        Some(height) => height,
        None => (cursor_y - content_y).max(0.0),
    };
    let content_height = clamp_dimension(
        content_height,
        style.min_height,
        style.max_height,
        containing.height.unwrap_or(ctx.viewport_height),
        font_size,
        root,
    );

    LayoutBox {
        style,
        rect: Rect::new(
            border_x,
            border_y,
            content_width + extras,
            content_height + padding.vertical() + border.vertical(),
        ),
        padding,
        items,
    }
}

/// Lays out the in-flow children of a block container, stacking block-level
/// children and collecting inline runs into lines.
///
/// Returns the paint items in order and the cursor after the last child.
fn layout_children(
    b: &BoxNode,
    ctx: &Ctx<'_>,
    content_x: f32,
    content_y: f32,
    content_width: f32,
    child_containing_height: Option<f32>,
    text_align: TextAlign,
) -> (Vec<PaintItem>, f32) {
    let root = ctx.root_font_size;
    let mut items: Vec<PaintItem> = Vec::new();
    let mut cursor_y = content_y;
    let mut previous_margin_bottom = 0.0_f32;
    let mut index = 0;
    while index < b.children.len() {
        let child = &b.children[index];
        if is_block_level(child) {
            let child_font_size = child.style.font_size;
            let child_margin_top = dimension_or_zero(
                child.style.margin.top,
                content_width,
                child_font_size,
                root,
            );
            let gap = previous_margin_bottom.max(child_margin_top);
            let child_containing = Containing {
                width: content_width,
                height: child_containing_height,
                x: content_x,
                // `layout_block` positions the border box at `containing.y`
                // plus the child's own top margin, so hand it the margin edge
                // the parent already collapsed.
                y: cursor_y + gap - child_margin_top,
            };
            let child_layout = if matches!(child.kind, BoxKind::Flex) {
                crate::flex::layout_flex(child, ctx, child_containing)
            } else {
                layout_block(child, ctx, child_containing, None)
            };
            cursor_y = child_layout.rect.bottom();
            previous_margin_bottom = dimension_or_zero(
                child.style.margin.bottom,
                content_width,
                child_font_size,
                root,
            );
            items.push(PaintItem::Box(Box::new(child_layout)));
            index += 1;
        } else {
            let start = index;
            while index < b.children.len() && !is_block_level(&b.children[index]) {
                index += 1;
            }
            let run = &b.children[start..index];
            let inline =
                layout_inline_run(run, ctx, content_x, cursor_y, content_width, text_align);
            cursor_y += inline.height;
            previous_margin_bottom = 0.0;
            items.extend(inline.items);
        }
    }
    (items, cursor_y)
}

/// The result of laying out one run of inline-level children.
struct InlineResult {
    /// Total height of the line boxes.
    height: f32,
    /// Text runs and atomic boxes in paint order.
    items: Vec<PaintItem>,
}

/// One inline-level item collected before line breaking.
#[expect(
    clippy::large_enum_variant,
    reason = "styles stay inline Copy data; boxing every word would allocate per token"
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
fn layout_inline_run(
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
    reason = "styles stay inline Copy data; boxing line items would allocate per word"
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
    if let Some(LineItem::Space { width }) = line.items.pop() {
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
                let containing = Containing {
                    width: measurement.content_width,
                    height: None,
                    x: cursor,
                    y: baseline - measurement.height,
                };
                let layout = layout_block(node, ctx, containing, Some(measurement.content_width));
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
        BoxKind::InlineBlock | BoxKind::InlineFlex | BoxKind::Block | BoxKind::Flex
        | BoxKind::ListItem => {
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
                style: *style,
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

/// Measures an atomic box at its max-content width.
fn measure_atomic(node: &BoxNode, ctx: &Ctx<'_>) -> Atomic {
    let preferred = max_content_width(node, ctx);
    let layout = layout_block(
        node,
        ctx,
        Containing {
            width: preferred,
            height: None,
            x: 0.0,
            y: 0.0,
        },
        Some((preferred - outer_extras(node, ctx)).max(0.0)),
    );
    let margin = resolve_margins(
        node.style.margin,
        preferred,
        node.style.font_size,
        ctx.root_font_size,
    );
    Atomic {
        content_width: layout.content_box().width,
        height: layout.rect.height,
        outer_width: layout.rect.width + margin.left + margin.right,
    }
}

/// Padding plus border of a box: the difference between content and border
/// box.
fn outer_extras(node: &BoxNode, ctx: &Ctx<'_>) -> f32 {
    let style = node.style;
    let padding = style.padding.left.resolve(0.0, style.font_size, ctx.root_font_size)
        + style.padding.right.resolve(0.0, style.font_size, ctx.root_font_size);
    padding + style.border.left.width + style.border.right.width
}

/// Max-content width of a box
/// (<https://drafts.csswg.org/css-sizing-3/#max-content>).
pub(crate) fn max_content_width(node: &BoxNode, ctx: &Ctx<'_>) -> f32 {
    let style = node.style;
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
        BoxKind::InlineBlock => node
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

/// Resolves margins, treating `auto` as zero.
fn resolve_margins(margin: Edges<Dimension>, basis: f32, font_size: f32, root: f32) -> Edges<f32> {
    margin.map(|dimension| dimension_or_zero(dimension, basis, font_size, root))
}

/// One box's resolved outer and inner spacing.
struct BoxMetrics {
    /// Resolved margins (`auto` as zero).
    margin: Edges<f32>,
    /// Resolved padding.
    padding: Edges<f32>,
    /// Border widths.
    border: Edges<f32>,
    /// Horizontal padding plus border.
    extras: f32,
}

/// Resolves margin, padding, and border widths for one style.
fn box_metrics(style: &Style, basis: f32, font_size: f32, root: f32) -> BoxMetrics {
    let margin = resolve_margins(style.margin, basis, font_size, root);
    let padding = style
        .padding
        .map(|length| length.resolve(basis, font_size, root));
    let border = Edges::new(
        style.border.top.width,
        style.border.right.width,
        style.border.bottom.width,
        style.border.left.width,
    );
    BoxMetrics {
        margin,
        padding,
        border,
        extras: padding.horizontal() + border.horizontal(),
    }
}

fn dimension_or_zero(dimension: Dimension, basis: f32, font_size: f32, root: f32) -> f32 {
    dimension_value(dimension, basis, font_size, root).unwrap_or(0.0)
}

/// Resolves a dimension, treating `auto` as absent.
fn dimension_value(
    dimension: Dimension,
    basis: f32,
    font_size: f32,
    root: f32,
) -> Option<f32> {
    match dimension {
        Dimension::Auto => None,
        Dimension::Length(length) => Some(length.resolve(basis, font_size, root)),
    }
}

/// Horizontal auto margins, per CSS 2.1 10.3.3.
fn resolve_auto_margins(
    declared: Edges<Dimension>,
    resolved: Edges<f32>,
    width: Dimension,
    basis: f32,
    outer_width: f32,
) -> (f32, f32) {
    let auto_left = declared.left == Dimension::Auto;
    let auto_right = declared.right == Dimension::Auto;
    let slack = basis - outer_width;
    match (width, auto_left, auto_right) {
        (Dimension::Length(_), true, true) => {
            let half = (slack / 2.0).max(0.0);
            (half, half)
        }
        (Dimension::Length(_), true, false) => (slack.max(0.0), resolved.right),
        (Dimension::Length(_), false, true) => (resolved.left, slack.max(0.0)),
        _ => (resolved.left, resolved.right),
    }
}

/// Applies min/max constraints to a content-box dimension.
fn clamp_dimension(
    value: f32,
    min: Dimension,
    max: Dimension,
    basis: f32,
    font_size: f32,
    root: f32,
) -> f32 {
    let min = match min {
        Dimension::Auto => 0.0,
        Dimension::Length(length) => length.resolve(basis, font_size, root).max(0.0),
    };
    let max = match max {
        Dimension::Auto => f32::INFINITY,
        Dimension::Length(length) => length.resolve(basis, font_size, root).max(0.0),
    };
    value.max(min).min(max)
}
