//! Inline formatting through Parley, atomic boxes through nested subtrees.
//!
//! One inline run becomes one Parley layout: text segments keep their styles,
//! atomic boxes become inline boxes at byte offsets, and Parley breaks,
//! shapes, and aligns. Paint receives positioned glyph runs, never strings.

use dom::NodeId;

use crate::font::Fonts;
use crate::geometry::{Edges, Rect};
use crate::style::{BoxSizing, Dimension, Style, TextAlign, WhiteSpace};
use crate::text::{FontStyle, PlacedRun, Segment, SegmentStyle};
use crate::tree::{BoxKind, BoxNode};

/// One painted child in tree order.
pub(crate) enum PaintItem {
    /// A nested box.
    Box(Box<LayoutBox>),
    /// A shaped glyph run.
    Glyphs(PlacedRun),
}

/// A laid-out box.
pub(crate) struct LayoutBox {
    /// Computed style of the originating element (or the anonymous box).
    pub(crate) style: Style,
    /// The DOM element this box was generated for, when it has one.
    pub(crate) node: Option<NodeId>,
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
            self.rect.height
                - border.top.width
                - border.bottom.width
                - padding.top
                - padding.bottom,
        )
    }
}

/// Per-layout context: the embedded faces.
pub(crate) struct Ctx<'a> {
    /// Embedded faces.
    pub(crate) fonts: &'a Fonts,
}

/// The result of laying out one run of inline-level children.
pub(crate) struct InlineResult {
    /// Total height of the line boxes.
    pub(crate) height: f32,
    /// Glyph runs and atomic boxes in paint order.
    pub(crate) items: Vec<PaintItem>,
}

/// Measured atomic box.
#[derive(Clone, Copy, Debug)]
struct Atomic {
    /// Border-box height.
    height: f32,
    /// Outer width including margins.
    outer_width: f32,
    /// Content width used to lay the box out.
    content_width: f32,
}

/// One collected input token in document order.
enum RawToken<'a> {
    /// Text with its segment style.
    Text(String, SegmentStyle),
    /// An atomic box with its measurements.
    Atomic {
        node: &'a BoxNode,
        measurement: Atomic,
    },
    /// A forced line break (`<br>`).
    Break,
}

/// Lays out `run` into lines starting at `(x, y)`.
///
/// Text is shaped by Parley; atomic boxes are laid out in nested subtrees
/// and placed at Parley's inline-box positions.
pub(crate) fn layout_inline_run(
    run: &[BoxNode],
    ctx: &Ctx<'_>,
    x: f32,
    y: f32,
    available: f32,
    text_align: TextAlign,
) -> InlineResult {
    let mut raw: Vec<RawToken<'_>> = Vec::new();
    // Parley has one whitespace mode per layout; preserve wins and collapsing
    // runs are pre-collapsed below, so mixed modes stay exact.
    let mut preserve = false;
    // Wrapping is disabled only when every text node refuses to wrap.
    let mut wrap = false;
    // Collapsible whitespace belongs to the whole run: a space at the end of
    // one text box waits for a following token instead of vanishing.
    let mut pending_space = false;
    for node in run {
        collect_token(
            node,
            ctx,
            &mut raw,
            &mut preserve,
            &mut wrap,
            &mut pending_space,
        );
    }
    if raw.is_empty() {
        return InlineResult {
            height: 0.0,
            items: Vec::new(),
        };
    }
    // Forced breaks split the run into separate layouts; each stacks below
    // the last. An empty group still occupies a strut line.
    let mut items: Vec<PaintItem> = Vec::new();
    let mut height = 0.0_f32;
    let mut group: Vec<&RawToken<'_>> = Vec::new();
    let mut strut = 0.0_f32;
    for token in raw.iter().chain(std::iter::once(&BREAK_SENTINEL)) {
        if matches!(token, RawToken::Break) {
            height += shape_group(
                &group,
                ctx,
                x,
                y + height,
                available,
                text_align,
                preserve,
                wrap,
                strut,
                &mut items,
            );
            group.clear();
            continue;
        }
        if let RawToken::Text(_, style) = token {
            strut = strut.max(style.size);
        }
        group.push(token);
    }
    InlineResult { height, items }
}

/// Sentinel that flushes the final group.
const BREAK_SENTINEL: RawToken<'static> = RawToken::Break;

/// Shapes one break-free group and appends its items; returns its height.
#[expect(
    clippy::too_many_arguments,
    reason = "one group carries the leaf's full shaping context; the parameters are the leaf's own"
)]
fn shape_group(
    group: &[&RawToken<'_>],
    ctx: &Ctx<'_>,
    x: f32,
    y: f32,
    available: f32,
    text_align: TextAlign,
    preserve: bool,
    wrap: bool,
    strut: f32,
    items: &mut Vec<PaintItem>,
) -> f32 {
    if group.is_empty() {
        return strut * 1.2;
    }
    let mut tokens: Vec<Segment<'_>> = Vec::new();
    let mut atomics: Vec<(&BoxNode, Atomic)> = Vec::new();
    for token in group {
        match token {
            RawToken::Text(text, style) => {
                tokens.push(Segment::Text(text, *style));
            }
            RawToken::Atomic { node, measurement } => {
                let index = atomics.len();
                atomics.push((*node, *measurement));
                tokens.push(Segment::Atomic(index));
            }
            RawToken::Break => {}
        }
    }
    let sizes: Vec<(f32, f32)> = atomics
        .iter()
        .map(|(_, measurement)| (measurement.outer_width, measurement.height))
        .collect();
    let width = if wrap { Some(available.max(0.0)) } else { None };
    let lines = crate::text::shape_lines(ctx.fonts, &tokens, &sizes, width, text_align, preserve);
    let mut height = 0.0_f32;
    for line in &lines {
        for atomic in &line.atomics {
            let (node, measurement) = atomics[atomic.index];
            let mut layout = crate::boxes::layout_subtree(node, ctx, measurement.content_width);
            shift_layout(&mut layout, x + atomic.x, y + height + atomic.y);
            items.push(PaintItem::Box(Box::new(layout)));
        }
        for glyph_run in &line.runs {
            let mut shifted = glyph_run.clone();
            shift_glyph_run(&mut shifted, x, y + height);
            items.push(PaintItem::Glyphs(shifted));
        }
        height += line.height;
    }
    height
}

/// Shifts one shaped run by `(dx, dy)`.
fn shift_glyph_run(run: &mut PlacedRun, dx: f32, dy: f32) {
    run.x += dx;
    run.baseline += dy;
    for glyph in &mut run.glyphs {
        glyph.x += dx;
        glyph.y += dy;
    }
}

/// Collects one box's text and atomics; `preserve` and `wrap` accumulate the
/// run's Parley modes.
fn collect_token<'a>(
    node: &'a BoxNode,
    ctx: &Ctx<'_>,
    out: &mut Vec<RawToken<'a>>,
    preserve: &mut bool,
    wrap: &mut bool,
    pending_space: &mut bool,
) {
    match &node.kind {
        BoxKind::Text(text) => {
            let processed = process_text(text, &node.style);
            *preserve |= processed.preserve;
            *wrap |= allows_wrap(&node.style);
            if !processed.text.is_empty() {
                if *pending_space || processed.leading_space {
                    // Parley trims the leading whitespace of every style span,
                    // so a separating space only survives on the previous
                    // token's trailing edge
                    // (<https://drafts.csswg.org/css-text-3/#white-space-processing>).
                    if let Some(RawToken::Text(previous, _)) = out.last_mut() {
                        previous.push(' ');
                    }
                }
                out.push(RawToken::Text(
                    processed.text,
                    SegmentStyle::from_style(&node.style),
                ));
            }
            *pending_space = processed.trailing_space;
        }
        BoxKind::Inline => {
            for child in &node.children {
                collect_token(child, ctx, out, preserve, wrap, pending_space);
            }
        }
        // `<br>` always breaks, in every whitespace mode. It splits the run
        // into separate Parley layouts below; a pending space is trailing
        // whitespace at the break and collapses away.
        BoxKind::Break => {
            *pending_space = false;
            *wrap = true;
            out.push(RawToken::Break);
        }
        BoxKind::InlineBlock
        | BoxKind::InlineFlex
        | BoxKind::InlineGrid
        | BoxKind::Block
        | BoxKind::Flex
        | BoxKind::Grid
        | BoxKind::ListItem => {
            // A space before an inline atomic still paints; keep it on the
            // preceding text token.
            if *pending_space {
                if let Some(RawToken::Text(previous, _)) = out.last_mut() {
                    previous.push(' ');
                }
                *pending_space = false;
            }
            let measurement = measure_atomic(node, ctx);
            out.push(RawToken::Atomic { node, measurement });
        }
    }
}

/// Whether text with this style wraps at soft opportunities.
///
/// `nowrap` and `pre` only break where the markup does; `pre-wrap` wraps while
/// preserving spaces
/// (<https://drafts.csswg.org/css-text-3/#white-space-property>).
fn allows_wrap(style: &Style) -> bool {
    matches!(style.white_space, WhiteSpace::Normal | WhiteSpace::PreWrap)
}

/// One text box after case mapping and whitespace processing.
struct ProcessedText {
    /// Text with leading and trailing collapsible runs removed.
    text: String,
    /// The box began with collapsible whitespace.
    leading_space: bool,
    /// The box ended with collapsible whitespace.
    trailing_space: bool,
    /// The box preserves whitespace (`white-space: pre` or `pre-wrap`).
    preserve: bool,
}

/// Pre-processes text for Parley: case mapping, then whitespace handling per
/// `white-space`. Collapsing runs are squeezed here so the builder can run in
/// preserve mode; leading and trailing runs are reported so the caller can
/// collapse them across text boxes.
fn process_text(text: &str, style: &Style) -> ProcessedText {
    let text = match style.text_transform {
        crate::style::TextTransform::None => text.to_owned(),
        crate::style::TextTransform::Uppercase => text.to_uppercase(),
        crate::style::TextTransform::Lowercase => text.to_lowercase(),
    };
    let preserve = matches!(style.white_space, WhiteSpace::Pre | WhiteSpace::PreWrap);
    if preserve {
        return ProcessedText {
            text,
            leading_space: false,
            trailing_space: false,
            preserve: true,
        };
    }
    let mut out = String::with_capacity(text.len());
    let mut leading_space = false;
    let mut pending = false;
    for ch in text.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
            pending = true;
        } else {
            if pending {
                if out.is_empty() {
                    leading_space = true;
                } else {
                    out.push(' ');
                }
                pending = false;
            }
            out.push(ch);
        }
    }
    ProcessedText {
        text: out,
        leading_space,
        trailing_space: pending,
        preserve: false,
    }
}
/// tree rooted at the atomic.
fn measure_atomic(node: &BoxNode, ctx: &Ctx<'_>) -> Atomic {
    let preferred = max_content_width(node, ctx);
    let layout = crate::boxes::layout_subtree(node, ctx, preferred);
    let margin = node.style.margin.map(|dimension| match dimension {
        Dimension::Auto => 0.0,
        Dimension::Length(length) => length.resolve(preferred),
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
            PaintItem::Glyphs(run) => shift_glyph_run(run, dx, dy),
        }
    }
}

/// Measures text with optional letter spacing, for intrinsic widths.
fn measure(text: &str, font: FontStyle, letter_spacing: f32, fonts: &Fonts) -> f32 {
    let base = fonts.measure(text, font);
    if letter_spacing == 0.0 {
        base
    } else {
        base + letter_spacing * crate::count(text.chars().count())
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
        BoxKind::InlineBlock
        | BoxKind::InlineFlex
        | BoxKind::InlineGrid
        | BoxKind::Block
        | BoxKind::Flex
        | BoxKind::Grid
        | BoxKind::ListItem => max_content_width(node, ctx),
    }
}

/// Padding plus border of a box: the difference between content and border
/// box.
fn outer_extras(node: &BoxNode) -> f32 {
    let style = &node.style;
    let padding = style.padding.left.resolve(0.0) + style.padding.right.resolve(0.0);
    padding + style.border.left.width + style.border.right.width
}

/// Max-content width of a box
/// (<https://drafts.csswg.org/css-sizing-3/#max-content>).
pub(crate) fn max_content_width(node: &BoxNode, ctx: &Ctx<'_>) -> f32 {
    let style = &node.style;
    let extras = outer_extras(node);
    let content = match &node.kind {
        BoxKind::Text(text) => {
            let font = FontStyle {
                size: style.font_size,
                weight: style.font_weight,
            };
            // Max-content is the whole unwrapped line, so accumulate the line
            // and take the widest one at forced breaks; `process_text` applies
            // the same case mapping and whitespace collapsing as shaping.
            let mapped = process_text(text, style).text;
            let mut width = 0.0_f32;
            let mut current = 0.0_f32;
            for ch in mapped.chars() {
                if ch == '\n' {
                    width = width.max(current);
                    current = 0.0;
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
                crate::style::FlexDirection::Row | crate::style::FlexDirection::RowReverse
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
            let value = length.resolve(0.0);
            if style.box_sizing == BoxSizing::BorderBox {
                value
            } else {
                value + extras
            }
        }
        Dimension::Auto => content + extras,
    }
}
