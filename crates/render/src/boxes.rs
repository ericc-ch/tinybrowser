//! Box layout through Taffy: block containers, flex, floats, and positioned
//! boxes.
//!
//! Taffy owns every box-level decision (block flow with margin collapsing,
//! the flex algorithm, float placement, absolute positioning); this crate
//! keeps the cascade, the box tree, inline formatting, and paint. The split
//! mirrors Blitz, which pairs Taffy layout with its own paint: the layout
//! engine positions border boxes, and inline content is measured through
//! Taffy's measure hooks.
//!
//! Inline formatting contexts (anonymous block boxes and blockified flex
//! items) become measured leaves: the measure function runs our inline
//! engine at the width Taffy offers and returns the height. Atomic
//! inline-blocks recurse through a nested Taffy tree.
//!
//! Two deliberate deviations live here: Taffy 0.14 dropped the CSS `order`
//! property, so flex children are stable-sorted by `order` at tree-build
//! time; and text does not wrap around floats yet (leaves measure at full
//! width), so floated boxes can overlap inline content.

use std::collections::HashMap;

use taffy::style_helpers::{FromLength as _, FromPercent as _, TaffyAuto as _};
use taffy::tree::{LayoutInput, LayoutOutput};
use taffy::{
    AlignContent as TaffyAlignContent, AlignItems as TaffyAlignItems,
    AvailableSpace as TaffyAvailableSpace, Display as TaffyDisplay,
    FlexDirection as TaffyFlexDirection, FlexWrap as TaffyFlexWrap, Float as TaffyFloat,
    JustifyContent as TaffyJustifyContent, Overflow as TaffyOverflow, Position as TaffyPosition,
    Rect as TaffyRect, Size as TaffySize, Style as TaffyStyle, TaffyTree,
};

use crate::font::Fonts;
use crate::geometry::{Edges, Rect};
use crate::layout::{Ctx, LayoutBox, PaintItem, layout_inline_run, min_content_width};
use crate::style::{
    AlignContent, AlignItems, AlignSelf, BoxSizing, Clear, Dimension, Display, FlexDirection,
    FlexWrap, Float, JustifyContent, Length, Overflow, Position, Style, TextAlign,
};
use crate::tree::{BoxKind, BoxNode, is_block_level};

/// One inline formatting context owned by a Taffy leaf.
#[derive(Clone, Copy)]
struct TextLeaf {
    /// Index into the builder's inline runs.
    run: usize,
}

/// One run of inline-level boxes laid out as a unit.
struct InlineRun<'a> {
    /// The boxes in tree order.
    nodes: &'a [BoxNode],
    /// Horizontal alignment of short lines.
    text_align: TextAlign,
}

/// Per-node data kept alongside the Taffy tree for the convert pass.
struct NodeData {
    /// Computed style of the originating box.
    style: Style,
    /// Inline run when this node is a measured leaf.
    leaf: Option<usize>,
}

/// Builds one Taffy tree per render and converts the result back.
struct Builder<'a> {
    /// The Taffy tree under construction.
    tree: TaffyTree<TextLeaf>,
    /// Taffy node to our data.
    data: HashMap<taffy::NodeId, NodeData>,
    /// Inline runs owned by leaves, indexed by [`TextLeaf::run`].
    runs: Vec<InlineRun<'a>>,
    /// Inline layout results cached by the measure function, with the width
    /// they were laid out at.
    cache: HashMap<taffy::NodeId, (f32, Vec<PaintItem>)>,
    /// Layout fonts and sizes.
    ctx: &'a Ctx<'a>,
}

/// Lays out the root wrapper through Taffy.
pub(crate) fn layout_root(root: &BoxNode, fonts: &Fonts, viewport_width: f32, viewport_height: f32) -> LayoutBox {
    let ctx = Ctx {
        fonts,
        root_font_size: root.style.font_size,
    };
    let mut builder = Builder::new(&ctx);
    let root_id = builder.build_node(root);
    let available = TaffySize {
        width: TaffyAvailableSpace::Definite(viewport_width),
        height: TaffyAvailableSpace::MaxContent,
    };
    // The measure closure borrows only runs/cache/ctx, disjoint from the
    // tree it measures into.
    let Builder {
        tree,
        runs,
        cache,
        data,
        ctx,
        ..
    } = &mut builder;
    tree.compute_layout_with_measure(root_id, available, |input, node, context, _| {
        measure_leaf(runs, cache, data, ctx, input, node, context)
    })
    .expect("taffy layout cannot fail on a tree we built");
    let layout = builder.convert(root_id, 0.0, 0.0, viewport_width);
    Builder::finish_root(layout, viewport_height)
}

/// Lays out one atomic subtree (an inline-block/flex) at a fixed available
/// width, for inline-level measurement and placement.
pub(crate) fn layout_subtree(node: &BoxNode, ctx: &Ctx<'_>, available: f32) -> LayoutBox {
    let mut builder = Builder::new(ctx);
    let root_id = builder.build_node(node);
    let space = TaffySize {
        width: TaffyAvailableSpace::Definite(available.max(0.0)),
        height: TaffyAvailableSpace::MaxContent,
    };
    let Builder {
        tree,
        runs,
        cache,
        data,
        ctx,
        ..
    } = &mut builder;
    tree.compute_layout_with_measure(root_id, space, |input, id, context, _| {
        measure_leaf(runs, cache, data, ctx, input, id, context)
    })
    .expect("taffy layout cannot fail on a tree we built");
    builder.convert(root_id, 0.0, 0.0, available)
}

/// Measures one text leaf with our inline engine.
fn measure_leaf(
    runs: &[InlineRun<'_>],
    cache: &mut HashMap<taffy::NodeId, (f32, Vec<PaintItem>)>,
    data: &HashMap<taffy::NodeId, NodeData>,
    ctx: &Ctx<'_>,
    input: LayoutInput,
    node: taffy::NodeId,
    context: Option<&mut TextLeaf>,
) -> LayoutOutput {
    let (width, height) = match context {
        Some(leaf) => {
            let run = &runs[leaf.run];
            let width = if let Some(width) = input.known_dimensions.width {
                width
            } else {
                match input.available_space.width {
                    TaffyAvailableSpace::Definite(width) => width,
                    TaffyAvailableSpace::MinContent => {
                        return output(min_content_width(run.nodes, ctx), 0.0);
                    }
                    TaffyAvailableSpace::MaxContent => {
                        return output(run_max_content(run.nodes, ctx), 0.0);
                    }
                }
            };
            let result =
                layout_inline_run(run.nodes, ctx, 0.0, 0.0, width.max(0.0), run.text_align);
            let height = match input.known_dimensions.height {
                Some(height) => height,
                None => result.height,
            };
            cache.insert(node, (width, result.items));
            (width, height)
        }
        None => {
            // A childless element box (empty divs, floats): Taffy still
            // routes these through measure, so resolve the style size here
            // instead of reporting zero.
            if let Some(known) = data.get(&node) {
                styled_size(&known.style, ctx, &input)
            } else {
                (
                    input.known_dimensions.width.unwrap_or(0.0),
                    input.known_dimensions.height.unwrap_or(0.0),
                )
            }
        }
    };
    output(width, height)
}

/// Resolves an empty element box's style size for the measure fallback.
/// Percentages resolve against the parent size when Taffy supplies one.
fn styled_size(style: &Style, ctx: &Ctx<'_>, input: &LayoutInput) -> (f32, f32) {
    let basis = |axis: Option<f32>, available: TaffyAvailableSpace| {
        axis
            .or(match available {
                TaffyAvailableSpace::Definite(value) => Some(value),
                TaffyAvailableSpace::MinContent | TaffyAvailableSpace::MaxContent => None,
            })
            .unwrap_or(0.0)
    };
    let basis_w = basis(input.parent_size.width, input.available_space.width);
    let basis_h = basis(input.parent_size.height, input.available_space.height);
    let resolve = |dimension: Dimension, basis: f32| match dimension {
        Dimension::Auto => None,
        Dimension::Length(length) => {
            Some(length.resolve(basis, style.font_size, ctx.root_font_size))
        }
    };
    let width = input
        .known_dimensions
        .width
        .or_else(|| resolve(style.width, basis_w))
        .or(match input.available_space.width {
            TaffyAvailableSpace::Definite(width) => Some(width),
            TaffyAvailableSpace::MinContent | TaffyAvailableSpace::MaxContent => None,
        })
        .unwrap_or(0.0);
    let height = input
        .known_dimensions
        .height
        .or_else(|| resolve(style.height, basis_h))
        .unwrap_or(0.0);
    (width.max(0.0), height.max(0.0))
}

/// A [`LayoutOutput`] with the content rect covering the size. Leaves carry
/// no collapsible margins of their own.
fn output(width: f32, height: f32) -> LayoutOutput {
    LayoutOutput::from_sizes(
        TaffySize { width, height },
        TaffyRect {
            left: 0.0,
            top: 0.0,
            right: width,
            bottom: height,
        },
    )
}

impl<'a> Builder<'a> {
    /// An empty builder over `ctx`.
    fn new(ctx: &'a Ctx<'a>) -> Self {
        Self {
            tree: TaffyTree::new(),
            data: HashMap::new(),
            runs: Vec::new(),
            cache: HashMap::new(),
            ctx,
        }
    }

    /// Adds `node` and its subtree, returning the Taffy node.
    fn build_node(&mut self, node: &'a BoxNode) -> taffy::NodeId {
        match &node.kind {
            BoxKind::Flex => {
                let style = convert_style(&node.style, self.ctx.root_font_size);
                // Flex items are blockified: every child becomes its own item
                // (<https://drafts.csswg.org/css-flexbox-1/#flex-items>), in
                // `order` (<https://drafts.csswg.org/css-flexbox-1/#order-property>).
                let default_align = map_align(node.style.align_items);
                let mut children: Vec<&BoxNode> = node.children.iter().collect();
                children.sort_by_key(|child| child.style.order);
                let mut ids = Vec::with_capacity(children.len());
                for child in children {
                    let id = if is_block_level(child) && child.style.float == Float::None {
                        self.build_node(child)
                    } else if matches!(
                        child.kind,
                        BoxKind::InlineBlock | BoxKind::InlineFlex
                    ) {
                        // Blockified, keeping the item's own box properties.
                        self.build_leaf(
                            std::slice::from_ref(child),
                            node.style.text_align,
                            &child.style,
                        )
                    } else {
                        // Bare text and spans: neutral box, inherited text.
                        // The leaf style is a local; `build_leaf` copies it.
                        let neutral = neutral_item_style(&node.style);
                        self.build_leaf(
                            std::slice::from_ref(child),
                            node.style.text_align,
                            &neutral,
                        )
                    };
                    // `align-self` resolves per item; `auto` inherits.
                    if child.style.align_self != AlignSelf::Auto {
                        let resolved = resolve_align_self(child.style.align_self, default_align);
                        if let Ok(mut item) = self.tree.style(id).cloned() {
                            item.align_self = Some(resolved);
                            let _ = self.tree.set_style(id, item);
                        }
                    }
                    ids.push(id);
                }
                self.parent_node(style, &node.style, &ids)
            }
            BoxKind::Block | BoxKind::ListItem => {
                let inline_only = !node.children.is_empty()
                    && node.children.iter().all(|child| {
                        !is_block_level(child) || child.style.float != Float::None
                    });
                if inline_only {
                    // An inline formatting context: one measured leaf.
                    self.build_leaf(&node.children, node.style.text_align, &node.style)
                } else {
                    let style = convert_style(&node.style, self.ctx.root_font_size);
                    let mut ids = Vec::with_capacity(node.children.len());
                    for child in &node.children {
                        ids.push(self.build_node(child));
                    }
                    self.parent_node(style, &node.style, &ids)
                }
            }
            // Inline-level boxes only reach the builder as flex items or
            // defensive fallbacks; either way they form one run.
            BoxKind::Text(_)
            | BoxKind::Inline
            | BoxKind::InlineBlock
            | BoxKind::InlineFlex
            | BoxKind::Break => self.build_leaf(
                std::slice::from_ref(node),
                node.style.text_align,
                &node.style,
            ),
        }
    }

    /// Adds one parent node with converted style.
    fn parent_node(
        &mut self,
        style: TaffyStyle,
        ours: &Style,
        children: &[taffy::NodeId],
    ) -> taffy::NodeId {
        let id = self
            .tree
            .new_with_children(style, children)
            .expect("taffy tree accepts built children");
        self.data.insert(
            id,
            NodeData {
                style: *ours,
                leaf: None,
            },
        );
        id
    }

    /// Adds one measured leaf for an inline run.
    fn build_leaf(
        &mut self,
        run: &'a [BoxNode],
        text_align: TextAlign,
        style: &Style,
    ) -> taffy::NodeId {
        let run_index = self.runs.len();
        self.runs.push(InlineRun { nodes: run, text_align });
        let mut converted = convert_style(style, self.ctx.root_font_size);
        converted.display = TaffyDisplay::Block;
        let id = self
            .tree
            .new_leaf(converted)
            .expect("taffy tree accepts a leaf");
        self.tree
            .set_node_context(id, Some(TextLeaf { run: run_index }))
            .expect("leaf exists");
        self.data.insert(
            id,
            NodeData {
                style: *style,
                leaf: Some(run_index),
            },
        );
        id
    }

    /// Converts one laid-out Taffy node and its subtree into a [`LayoutBox`].
    fn convert(
        &mut self,
        node: taffy::NodeId,
        x: f32,
        y: f32,
        parent_content_width: f32,
    ) -> LayoutBox {
        let layout = *self.tree.layout(node).expect("computed layout");
        let data = self.data.remove(&node).expect("node data");
        let padding = data.style.padding.map(|length| {
            length.resolve(parent_content_width, data.style.font_size, self.ctx.root_font_size)
        });
        let rect = Rect::new(
            x + layout.location.x,
            y + layout.location.y,
            layout.size.width,
            layout.size.height,
        );
        // The containing width for percentage padding below is this box's
        // content width.
        let border = data.style.border;
        let content_width = (rect.width
            - border.left.width
            - border.right.width
            - padding.left
            - padding.right)
            .max(0.0);
        let mut items = Vec::new();
        if let Some(run_index) = data.leaf {
            let run = &self.runs[run_index];
            // The measure pass usually laid this run out at the final width;
            // flex stretching can widen the leaf afterwards, so re-run the
            // inline engine when the widths disagree.
            let cached = self.cache.remove(&node);
            let fresh = match cached {
                Some((width, mut items))
                    if (width - content_width).abs() < 0.5 =>
                {
                    crate::layout::shift_items(&mut items, rect.x, rect.y);
                    Some(items)
                }
                _ => None,
            };
            items = fresh.unwrap_or_else(|| {
                layout_inline_run(
                    run.nodes,
                    self.ctx,
                    rect.x,
                    rect.y,
                    content_width,
                    run.text_align,
                )
                .items
            });
        } else {
            let children = self.tree.children(node).expect("children");
            for child in children {
                items.push(PaintItem::Box(Box::new(self.convert(
                    child,
                    rect.x,
                    rect.y,
                    content_width,
                ))));
            }
        }
        LayoutBox {
            style: data.style,
            rect,
            padding,
            items,
        }
    }

    /// Grows the root to at least the viewport, so the canvas always covers
    /// the window.
    fn finish_root(mut layout: LayoutBox, viewport_height: f32) -> LayoutBox {
        layout.rect = Rect::new(
            0.0,
            0.0,
            layout.rect.width,
            layout.rect.height.max(viewport_height),
        );
        layout
    }
}

/// Converts our computed style to a Taffy style. Font-relative units resolve
/// now; percentages stay symbolic for Taffy to resolve.
#[expect(
    clippy::too_many_lines,
    reason = "the style conversion table: one arm per mapped CSS longhand keeps the mapping auditable in one place"
)]
fn convert_style(style: &Style, root_font_size: f32) -> TaffyStyle {
    let font_size = style.font_size;
    TaffyStyle {
        display: match style.display {
            Display::None => TaffyDisplay::None,
            Display::Block | Display::Inline | Display::InlineBlock | Display::ListItem => {
                TaffyDisplay::Block
            }
            Display::Flex | Display::InlineFlex => TaffyDisplay::Flex,
        },
        position: match style.position {
            Position::Static | Position::Relative => TaffyPosition::Relative,
            // Fixed positions against the viewport; the screenshot viewport
            // is unscrolled, so the initial containing block matches.
            Position::Absolute | Position::Fixed => TaffyPosition::Absolute,
        },
        inset: TaffyRect {
            left: to_auto(style.inset_left, font_size, root_font_size),
            right: to_auto(style.inset_right, font_size, root_font_size),
            top: to_auto(style.inset_top, font_size, root_font_size),
            bottom: to_auto(style.inset_bottom, font_size, root_font_size),
        },
        size: TaffySize {
            width: to_dimension(style.width, font_size, root_font_size),
            height: to_dimension(style.height, font_size, root_font_size),
        },
        min_size: TaffySize {
            width: to_auto(style.min_width, font_size, root_font_size),
            height: to_auto(style.min_height, font_size, root_font_size),
        },
        max_size: TaffySize {
            width: to_auto(style.max_width, font_size, root_font_size),
            height: to_auto(style.max_height, font_size, root_font_size),
        },
        margin: TaffyRect {
            left: to_auto(style.margin.left, font_size, root_font_size),
            right: to_auto(style.margin.right, font_size, root_font_size),
            top: to_auto(style.margin.top, font_size, root_font_size),
            bottom: to_auto(style.margin.bottom, font_size, root_font_size),
        },
        padding: TaffyRect {
            left: to_length(style.padding.left, font_size, root_font_size),
            right: to_length(style.padding.right, font_size, root_font_size),
            top: to_length(style.padding.top, font_size, root_font_size),
            bottom: to_length(style.padding.bottom, font_size, root_font_size),
        },
        border: TaffyRect {
            left: border_width(style.border.left.width),
            right: border_width(style.border.right.width),
            top: border_width(style.border.top.width),
            bottom: border_width(style.border.bottom.width),
        },
        box_sizing: match style.box_sizing {
            BoxSizing::ContentBox => taffy::BoxSizing::ContentBox,
            BoxSizing::BorderBox => taffy::BoxSizing::BorderBox,
        },
        overflow: taffy::Point {
            x: to_overflow(style.overflow),
            y: to_overflow(style.overflow),
        },
        float: match style.float {
            Float::None => TaffyFloat::None,
            Float::Left => TaffyFloat::Left,
            Float::Right => TaffyFloat::Right,
        },
        clear: match style.clear {
            Clear::None => taffy::Clear::None,
            Clear::Left => taffy::Clear::Left,
            Clear::Right => taffy::Clear::Right,
            Clear::Both => taffy::Clear::Both,
        },
        flex_direction: match style.flex_direction {
            FlexDirection::Row => TaffyFlexDirection::Row,
            FlexDirection::RowReverse => TaffyFlexDirection::RowReverse,
            FlexDirection::Column => TaffyFlexDirection::Column,
            FlexDirection::ColumnReverse => TaffyFlexDirection::ColumnReverse,
        },
        flex_wrap: match style.flex_wrap {
            FlexWrap::Nowrap => TaffyFlexWrap::NoWrap,
            FlexWrap::Wrap => TaffyFlexWrap::Wrap,
            FlexWrap::WrapReverse => TaffyFlexWrap::WrapReverse,
        },
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        flex_basis: to_dimension(style.flex_basis, font_size, root_font_size),
        justify_content: Some(match style.justify_content {
            JustifyContent::FlexStart => TaffyJustifyContent::FLEX_START,
            JustifyContent::FlexEnd => TaffyJustifyContent::FLEX_END,
            JustifyContent::Center => TaffyJustifyContent::CENTER,
            JustifyContent::SpaceBetween => TaffyJustifyContent::SPACE_BETWEEN,
            JustifyContent::SpaceAround => TaffyJustifyContent::SPACE_AROUND,
            JustifyContent::SpaceEvenly => TaffyJustifyContent::SPACE_EVENLY,
        }),
        align_items: Some(map_align(style.align_items)),
        align_content: Some(match style.align_content {
            AlignContent::Stretch => TaffyAlignContent::STRETCH,
            AlignContent::FlexStart => TaffyAlignContent::FLEX_START,
            AlignContent::FlexEnd => TaffyAlignContent::FLEX_END,
            AlignContent::Center => TaffyAlignContent::CENTER,
            AlignContent::SpaceBetween => TaffyAlignContent::SPACE_BETWEEN,
            AlignContent::SpaceAround => TaffyAlignContent::SPACE_AROUND,
        }),
        gap: TaffySize {
            width: to_length(style.column_gap, font_size, root_font_size),
            height: to_length(style.row_gap, font_size, root_font_size),
        },
        ..TaffyStyle::default()
    }
}

/// A neutral flex-item style for bare text and spans: no box of its own,
/// text properties inherited from the container.
fn neutral_item_style(parent: &Style) -> Style {
    use crate::style::BorderSide;
    let mut style = Style::inherited_from(parent);
    style.display = Display::Block;
    style.border = Edges::new(
        BorderSide::NONE,
        BorderSide::NONE,
        BorderSide::NONE,
        BorderSide::NONE,
    );
    style
}

/// Maps `align-items` to Taffy, shared by containers and `align-self`
/// resolution. Baseline falls back to flex-start until leaves report
/// baselines.
fn map_align(value: AlignItems) -> TaffyAlignItems {
    match value {
        AlignItems::Stretch => TaffyAlignItems::STRETCH,
        AlignItems::Center => TaffyAlignItems::CENTER,
        AlignItems::FlexEnd => TaffyAlignItems::FLEX_END,
        // Baseline needs reported baselines, which leaves do not supply yet;
        // pack at the start like the old engine did.
        AlignItems::FlexStart | AlignItems::Baseline => TaffyAlignItems::FLEX_START,
    }
}

/// Resolves `align-self` against the container default: `auto` inherits.
fn resolve_align_self(value: AlignSelf, default: TaffyAlignItems) -> TaffyAlignItems {
    match value {
        AlignSelf::Auto => default,
        AlignSelf::Stretch => TaffyAlignItems::STRETCH,
        AlignSelf::Center => TaffyAlignItems::CENTER,
        AlignSelf::FlexEnd => TaffyAlignItems::FLEX_END,
        AlignSelf::FlexStart | AlignSelf::Baseline => TaffyAlignItems::FLEX_START,
    }
}

/// Converts a length, resolving font-relative units now. Percentages use
/// Taffy's 0..=1 range.
fn to_length(length: Length, font_size: f32, root_font_size: f32) -> taffy::LengthPercentage {
    match length {
        Length::Px(value) => taffy::LengthPercentage::length(value),
        Length::Percent(value) => taffy::LengthPercentage::percent(value / 100.0),
        Length::Em(value) => taffy::LengthPercentage::length(value * font_size),
        Length::Rem(value) => taffy::LengthPercentage::length(value * root_font_size),
    }
}

/// Converts a dimension; `auto` stays symbolic.
fn to_dimension(dimension: Dimension, font_size: f32, root_font_size: f32) -> taffy::Dimension {
    match dimension {
        Dimension::Auto => taffy::Dimension::AUTO,
        Dimension::Length(length) => {
            taffy::Dimension::from(to_length(length, font_size, root_font_size))
        }
    }
}

/// Converts a margin or inset side; `auto` stays symbolic.
fn to_auto(
    dimension: Dimension,
    font_size: f32,
    root_font_size: f32,
) -> taffy::LengthPercentageAuto {
    match dimension {
        Dimension::Auto => taffy::LengthPercentageAuto::AUTO,
        Dimension::Length(Length::Px(value)) => {
            taffy::LengthPercentageAuto::from_length(value)
        }
        Dimension::Length(Length::Percent(value)) => {
            taffy::LengthPercentageAuto::from_percent(value / 100.0)
        }
        Dimension::Length(length) => taffy::LengthPercentageAuto::from_length(
            length.resolve(0.0, font_size, root_font_size),
        ),
    }
}

/// A border width in pixels.
fn border_width(width: f32) -> taffy::LengthPercentage {
    taffy::LengthPercentage::length(width.max(0.0))
}

/// Maps overflow to Taffy's clipping behavior.
fn to_overflow(overflow: Overflow) -> TaffyOverflow {
    match overflow {
        Overflow::Visible => TaffyOverflow::Visible,
        Overflow::Hidden => TaffyOverflow::Hidden,
    }
}

/// Max-content width of one inline run.
fn run_max_content(nodes: &[BoxNode], ctx: &Ctx<'_>) -> f32 {
    nodes
        .iter()
        .map(|node| crate::layout::max_content_width(node, ctx))
        .sum()
}
