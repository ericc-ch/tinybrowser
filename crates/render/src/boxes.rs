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

use taffy::style_helpers::{FromFr as _, FromLength as _, FromPercent as _, TaffyAuto as _};
use taffy::tree::{LayoutInput, LayoutOutput};
use taffy::{
    AlignContent as TaffyAlignContent, AlignItems as TaffyAlignItems,
    AvailableSpace as TaffyAvailableSpace, Display as TaffyDisplay,
    FlexDirection as TaffyFlexDirection, FlexWrap as TaffyFlexWrap, Float as TaffyFloat,
    GridTemplateComponent, GridTemplateRepetition, JustifyContent as TaffyJustifyContent,
    MaxTrackSizingFunction, MinTrackSizingFunction, Overflow as TaffyOverflow,
    Position as TaffyPosition, Rect as TaffyRect, RepetitionCount, Size as TaffySize,
    Style as TaffyStyle, TaffyTree, TrackSizingFunction,
};

use crate::font::Fonts;
use crate::geometry::{Edges, Rect};
use crate::layout::{Ctx, LayoutBox, PaintItem, layout_inline_run, min_content_width};
use crate::style::{
    AlignContent, AlignItems, AlignSelf, BoxSizing, Clear, Dimension, Display, FlexDirection,
    FlexWrap, Float, GridLine, GridPlacement, GridTrack, JustifyContent, Length, Overflow,
    Position, Style, TextAlign, TrackSize,
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
    let ctx = Ctx { fonts };
    let mut builder = Builder::new(&ctx);
    let root_id = builder.build_node(root);
    let available = TaffySize {
        width: TaffyAvailableSpace::Definite(viewport_width),
        height: TaffyAvailableSpace::MaxContent,
    };
    builder.run_layout(root_id, available);
    let layout = builder.convert(root_id, 0.0, 0.0, viewport_width);
    Builder::finish_root(layout, viewport_height)
}

/// Lays out one atomic subtree (an inline-block/flex/grid) at a fixed
/// available width, for inline-level measurement and placement.
pub(crate) fn layout_subtree(node: &BoxNode, ctx: &Ctx<'_>, available: f32) -> LayoutBox {
    let mut builder = Builder::new(ctx);
    // Atomic roots lay out as containers, never as text leaves: routing them
    // through `build_node` would wrap them in a leaf and recurse forever.
    let root_id = match &node.kind {
        BoxKind::InlineBlock => builder.build_block(node),
        BoxKind::InlineFlex => builder.build_flex(node),
        BoxKind::InlineGrid => builder.build_grid(node),
        _ => builder.build_node(node),
    };
    let space = TaffySize {
        width: TaffyAvailableSpace::Definite(available.max(0.0)),
        height: TaffyAvailableSpace::MaxContent,
    };
    builder.run_layout(root_id, space);
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
                styled_size(&known.style, &input)
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
fn styled_size(style: &Style, input: &LayoutInput) -> (f32, f32) {
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
            Some(length.resolve(basis))
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

    /// Runs Taffy's layout over the built tree.
    ///
    /// One method owns the measure closure so both entry points instantiate
    /// Taffy's generic compute machinery exactly once (the closure type is
    /// part of that instantiation; two closures would double its code).
    fn run_layout(&mut self, root: taffy::NodeId, available: TaffySize<TaffyAvailableSpace>) {
        // The measure closure borrows only runs/cache/data/ctx, disjoint from
        // the tree it measures into.
        let Builder {
            tree,
            runs,
            cache,
            data,
            ctx,
            ..
        } = self;
        tree.compute_layout_with_measure(root, available, |input, node, context, _| {
            measure_leaf(runs, cache, data, ctx, input, node, context)
        })
        .expect("taffy layout cannot fail on a tree we built");
    }

    /// Adds `node` and its subtree, returning the Taffy node.
    fn build_node(&mut self, node: &'a BoxNode) -> taffy::NodeId {
        match &node.kind {
            BoxKind::Flex => self.build_flex(node),
            BoxKind::Grid => self.build_grid(node),
            BoxKind::Block | BoxKind::ListItem => self.build_block(node),
            // Inline-level boxes only reach the builder as flex/grid items or
            // defensive fallbacks; either way they form one run.
            BoxKind::Text(_)
            | BoxKind::Inline
            | BoxKind::InlineBlock
            | BoxKind::InlineFlex
            | BoxKind::InlineGrid
            | BoxKind::Break => self.build_leaf(
                std::slice::from_ref(node),
                node.style.text_align,
                &node.style,
            ),
        }
    }

    /// Adds one flex container and its blockified items.
    fn build_flex(&mut self, node: &'a BoxNode) -> taffy::NodeId {
        let style = convert_style(&node.style);
        // Flex items are blockified: every child becomes its own item
        // (<https://drafts.csswg.org/css-flexbox-1/#flex-items>), in
        // `order` (<https://drafts.csswg.org/css-flexbox-1/#order-property>).
        let default_align = map_align(node.style.align_items);
        let mut children: Vec<&BoxNode> = node.children.iter().collect();
        children.sort_by_key(|child| child.style.order);
        let mut ids = Vec::with_capacity(children.len());
        for child in children {
            let id = self.build_item(child, &node.style);
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

    /// Adds one grid container and its blockified items.
    fn build_grid(&mut self, node: &'a BoxNode) -> taffy::NodeId {
        let style = convert_style(&node.style);
        // Grid items are blockified like flex items
        // (<https://drafts.csswg.org/css-grid-1/#grid-items>).
        let mut children: Vec<&BoxNode> = node.children.iter().collect();
        children.sort_by_key(|child| child.style.order);
        let mut ids = Vec::with_capacity(children.len());
        for child in children {
            let id = self.build_item(child, &node.style);
            self.apply_grid_placement(id, &child.style, &node.style);
            ids.push(id);
        }
        self.parent_node(style, &node.style, &ids)
    }

    /// Adds one block container, or one measured leaf when it holds only
    /// inline content.
    fn build_block(&mut self, node: &'a BoxNode) -> taffy::NodeId {
        let inline_only = !node.children.is_empty()
            && node
                .children
                .iter()
                .all(|child| !is_block_level(child) || child.style.float != Float::None);
        if inline_only {
            // An inline formatting context: one measured leaf.
            self.build_leaf(&node.children, node.style.text_align, &node.style)
        } else {
            let style = convert_style(&node.style);
            let mut ids = Vec::with_capacity(node.children.len());
            for child in &node.children {
                ids.push(self.build_node(child));
            }
            self.parent_node(style, &node.style, &ids)
        }
    }

    /// Adds one flex/grid item: block-level children recurse, everything else
    /// becomes a measured leaf.
    fn build_item(&mut self, child: &'a BoxNode, container: &Style) -> taffy::NodeId {
        if is_block_level(child) && child.style.float == Float::None {
            self.build_node(child)
        } else if matches!(
            child.kind,
            BoxKind::InlineBlock | BoxKind::InlineFlex | BoxKind::InlineGrid
        ) {
            // Blockified, keeping the item's own box properties.
            self.build_leaf(
                std::slice::from_ref(child),
                container.text_align,
                &child.style,
            )
        } else {
            // Bare text and spans: neutral box, inherited text.
            // The leaf style is a local; `build_leaf` copies it.
            let neutral = neutral_item_style(container);
            self.build_leaf(std::slice::from_ref(child), container.text_align, &neutral)
        }
    }

    /// Applies one grid item's placement and self-alignment.
    fn apply_grid_placement(
        &mut self,
        id: taffy::NodeId,
        item: &Style,
        container: &Style,
    ) {
        let Ok(mut converted) = self.tree.style(id).cloned() else {
            return;
        };
        converted.grid_column = map_grid_line(item.grid_column);
        converted.grid_row = map_grid_line(item.grid_row);
        if item.align_self != AlignSelf::Auto {
            converted.align_self = Some(resolve_align_self(
                item.align_self,
                map_align(container.align_items),
            ));
        }
        let _ = self.tree.set_style(id, converted);
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
                style: ours.clone(),
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
        let mut converted = convert_style(style);
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
                style: style.clone(),
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
            length.resolve(parent_content_width)
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
fn convert_style(style: &Style) -> TaffyStyle {
    TaffyStyle {
        display: match style.display {
            Display::None => TaffyDisplay::None,
            Display::Block | Display::Inline | Display::InlineBlock | Display::ListItem => {
                TaffyDisplay::Block
            }
            Display::Flex | Display::InlineFlex => TaffyDisplay::Flex,
            Display::Grid | Display::InlineGrid => TaffyDisplay::Grid,
        },
        position: match style.position {
            Position::Static | Position::Relative => TaffyPosition::Relative,
            // Fixed positions against the viewport; the screenshot viewport
            // is unscrolled, so the initial containing block matches.
            Position::Absolute | Position::Fixed => TaffyPosition::Absolute,
        },
        inset: TaffyRect {
            left: to_auto(style.inset_left),
            right: to_auto(style.inset_right),
            top: to_auto(style.inset_top),
            bottom: to_auto(style.inset_bottom),
        },
        size: TaffySize {
            width: to_dimension(style.width),
            height: to_dimension(style.height),
        },
        min_size: TaffySize {
            width: to_auto(style.min_width),
            height: to_auto(style.min_height),
        },
        max_size: TaffySize {
            width: to_auto(style.max_width),
            height: to_auto(style.max_height),
        },
        margin: TaffyRect {
            left: to_auto(style.margin.left),
            right: to_auto(style.margin.right),
            top: to_auto(style.margin.top),
            bottom: to_auto(style.margin.bottom),
        },
        padding: TaffyRect {
            left: to_length(style.padding.left),
            right: to_length(style.padding.right),
            top: to_length(style.padding.top),
            bottom: to_length(style.padding.bottom),
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
        flex_basis: to_dimension(style.flex_basis),
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
            width: to_length(style.column_gap),
            height: to_length(style.row_gap),
        },
        grid_template_columns: map_tracks(&style.grid_template_columns),
        grid_template_rows: map_tracks(&style.grid_template_rows),
        justify_items: Some(map_align(style.justify_items)),
        ..TaffyStyle::default()
    }
}

/// Maps one axis placement.
fn map_grid_line(line: GridLine) -> taffy::Line<taffy::GridPlacement> {
    taffy::Line {
        start: map_placement(line.start),
        end: map_placement(line.end),
    }
}

/// Maps one line placement; negative lines count from the end.
fn map_placement(placement: GridPlacement) -> taffy::GridPlacement {
    match placement {
        GridPlacement::Auto => taffy::GridPlacement::Auto,
        GridPlacement::Line(number) => {
            let narrowed = number.clamp(i32::from(i16::MIN), i32::from(i16::MAX));
            #[expect(
                clippy::cast_possible_truncation,
                reason = "clamped to i16 range above; the cast is infallible"
            )]
            let narrowed = narrowed as i16;
            taffy::GridPlacement::Line(narrowed.into())
        }
        GridPlacement::Span(count) => taffy::GridPlacement::Span(count),
    }
}

/// Maps a template track list.
fn map_tracks(tracks: &[GridTrack]) -> Vec<GridTemplateComponent<String>> {
    let mut out = Vec::new();
    for track in tracks {
        push_track(track, &mut out);
    }
    out
}

/// Appends one track, expanding repeats.
fn push_track(track: &GridTrack, out: &mut Vec<GridTemplateComponent<String>>) {
    match track {
        GridTrack::Single(size) => out.push(GridTemplateComponent::<String>::Single(
            map_track_size(size),
        )),
        GridTrack::MinMax(min, max) => out.push(GridTemplateComponent::<String>::Single(TrackSizingFunction {
            min: map_min_size(min),
            max: map_max_size(max),
        })),
        GridTrack::Repeat(count, tracks) => {
            let mut repeated = Vec::with_capacity(tracks.len());
            for track in tracks {
                // Nested repeats are rejected at parse time; skip defensively.
                match track {
                    GridTrack::Single(size) => repeated.push(map_track_size(size)),
                    GridTrack::MinMax(min, max) => repeated.push(TrackSizingFunction {
                        min: map_min_size(min),
                        max: map_max_size(max),
                    }),
                    GridTrack::Repeat(_, _) => {}
                }
            }
            out.push(GridTemplateComponent::<String>::Repeat(GridTemplateRepetition {
                count: RepetitionCount::Count(*count),
                tracks: repeated,
                line_names: Vec::new(),
            }));
        }
    }
}

/// Maps one track size.
fn map_track_size(size: &TrackSize) -> TrackSizingFunction {
    match size {
        TrackSize::Auto => TrackSizingFunction::AUTO,
        TrackSize::Length(length) => {
            TrackSizingFunction::from(to_length(*length))
        }
        TrackSize::Flex(flex) => TrackSizingFunction::from_fr(*flex),
    }
}

/// Maps a min-track size; flexible fractions are invalid here and fall back
/// to `auto`.
fn map_min_size(size: &TrackSize) -> MinTrackSizingFunction {
    match size {
        TrackSize::Auto | TrackSize::Flex(_) => MinTrackSizingFunction::AUTO,
        TrackSize::Length(length) => {
            MinTrackSizingFunction::from(to_length(*length))
        }
    }
}

/// Maps a max-track size.
fn map_max_size(size: &TrackSize) -> MaxTrackSizingFunction {
    match size {
        TrackSize::Auto => MaxTrackSizingFunction::AUTO,
        TrackSize::Length(length) => {
            MaxTrackSizingFunction::from(to_length(*length))
        }
        TrackSize::Flex(flex) => MaxTrackSizingFunction::from_fr(*flex),
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

/// Converts a length. Percentages use Taffy's 0..=1 range; font-relative
/// units were already resolved by the cascade.
fn to_length(length: Length) -> taffy::LengthPercentage {
    match length {
        Length::Px(value) => taffy::LengthPercentage::length(value),
        Length::Percent(value) => taffy::LengthPercentage::percent(value / 100.0),
    }
}

/// Converts a dimension; `auto` stays symbolic.
fn to_dimension(dimension: Dimension) -> taffy::Dimension {
    match dimension {
        Dimension::Auto => taffy::Dimension::AUTO,
        Dimension::Length(length) => taffy::Dimension::from(to_length(length)),
    }
}

/// Converts a margin or inset side; `auto` stays symbolic.
fn to_auto(dimension: Dimension) -> taffy::LengthPercentageAuto {
    match dimension {
        Dimension::Auto => taffy::LengthPercentageAuto::AUTO,
        Dimension::Length(Length::Px(value)) => taffy::LengthPercentageAuto::from_length(value),
        Dimension::Length(Length::Percent(value)) => {
            taffy::LengthPercentageAuto::from_percent(value / 100.0)
        }
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
