//! Flex container layout.
//!
//! Implements the flex layout algorithm of CSS Flexbox 1
//! (<https://drafts.csswg.org/css-flexbox-1/#layout-algorithm>) in phases:
//! resolve the container box, resolve each item's base size, break lines,
//! resolve flexible lengths, then place items. The supported subset is row and
//! column, wrap and nowrap, grow/shrink, `justify-content`, `align-items` and
//! `align-self` (baseline treated as flex-start), and `gap`. Reverse
//! directions reverse the visual order; `align-content` packs lines at the
//! start.

use crate::geometry::{Edges, Rect};
use crate::layout::{
    Containing, Ctx, LayoutBox, PaintItem, layout_block_sized, max_content_width,
};
use crate::style::{
    AlignItems, AlignSelf, BoxSizing, Dimension, FlexDirection, FlexWrap, JustifyContent,
};
use crate::tree::BoxNode;

/// The container's resolved box, in content-box terms where needed.
struct Container {
    content_x: f32,
    content_y: f32,
    content_width: f32,
    content_height: f32,
    main_size: f32,
    cross_size: f32,
    cross_auto: bool,
    padding: Edges<f32>,
    extras: Edges<f32>,
    margin: Edges<f32>,
    row: bool,
    reverse: bool,
}

/// One flex item's resolved geometry.
struct Item<'a> {
    node: &'a BoxNode,
    margin: Edges<f32>,
    extras_main: f32,
    extras_cross: f32,
    base: f32,
    main: f32,
    grow: f32,
    shrink: f32,
}

/// Lays out one flex container.
pub(crate) fn layout_flex(b: &BoxNode, ctx: &Ctx<'_>, containing: Containing) -> LayoutBox {
    let style = b.style;
    let container = resolve_container(b, ctx, containing);
    let gap = gap_size(&style, ctx, &container);
    let wraps = style.flex_wrap != FlexWrap::Nowrap;
    let mut items = resolve_items(b, ctx, &container);
    let lines = break_lines(&items, &container, gap, wraps);
    for line in &lines {
        resolve_line(&mut items, line, &container, gap);
    }
    let (painted, total_cross) = place_lines(b, ctx, &container, &items, &lines, gap);

    let used_width = container.content_width;
    let used_height = if container.row {
        if style.height == Dimension::Auto {
            total_cross
        } else {
            container.content_height
        }
    } else if style.height == Dimension::Auto {
        total_cross
    } else {
        container.content_height
    };

    LayoutBox {
        style,
        rect: Rect::new(
            containing.x + container.margin.left,
            containing.y + container.margin.top,
            used_width + container.extras.left + container.extras.right,
            used_height + container.extras.top + container.extras.bottom,
        ),
        padding: container.padding,
        items: painted,
    }
}

/// Resolves the container's content box and axis sizes.
fn resolve_container(b: &BoxNode, ctx: &Ctx<'_>, containing: Containing) -> Container {
    let style = b.style;
    let font_size = style.font_size;
    let root = ctx.root_font_size;
    let row = matches!(
        style.flex_direction,
        FlexDirection::Row | FlexDirection::RowReverse
    );
    let reverse = matches!(
        style.flex_direction,
        FlexDirection::RowReverse | FlexDirection::ColumnReverse
    );
    let padding = style
        .padding
        .map(|length| length.resolve(containing.width, font_size, root));
    let border = Edges::new(
        style.border.top.width,
        style.border.right.width,
        style.border.bottom.width,
        style.border.left.width,
    );
    let extras = Edges::new(
        padding.top + border.top,
        padding.right + border.right,
        padding.bottom + border.bottom,
        padding.left + border.left,
    );
    let margin = style.margin.map(|dimension| match dimension {
        Dimension::Auto => 0.0,
        Dimension::Length(length) => length.resolve(containing.width, font_size, root),
    });

    let content_width = match style.width {
        Dimension::Length(length) => {
            let value = length.resolve(containing.width, font_size, root);
            if style.box_sizing == BoxSizing::BorderBox {
                (value - extras.left - extras.right).max(0.0)
            } else {
                value
            }
        }
        Dimension::Auto => {
            (containing.width - margin.left - margin.right - extras.left - extras.right).max(0.0)
        }
    };
    let content_height = match style.height {
        Dimension::Length(length) => {
            let value = length.resolve(
                containing.height.unwrap_or(ctx.viewport_height),
                font_size,
                root,
            );
            if style.box_sizing == BoxSizing::BorderBox {
                (value - extras.top - extras.bottom).max(0.0)
            } else {
                value
            }
        }
        Dimension::Auto => (containing.height.unwrap_or(ctx.viewport_height)
            - margin.top
            - margin.bottom
            - extras.top
            - extras.bottom)
            .max(0.0),
    };
    Container {
        content_x: containing.x + margin.left + extras.left,
        content_y: containing.y + margin.top + extras.top,
        content_width,
        content_height,
        main_size: if row {
            content_width
        } else {
            content_height
        },
        cross_size: if row {
            content_height
        } else {
            content_width
        },
        cross_auto: if row {
            style.height == Dimension::Auto
        } else {
            style.width == Dimension::Auto
        },
        padding,
        extras,
        margin,
        row,
        reverse,
    }
}

/// The main-axis and cross-axis `gap`.
fn gap_size(style: &crate::style::Style, ctx: &Ctx<'_>, container: &Container) -> f32 {
    let gap = if container.row {
        style.column_gap
    } else {
        style.row_gap
    };
    gap.resolve(
        if container.row {
            container.content_width
        } else {
            container.content_height
        },
        style.font_size,
        ctx.root_font_size,
    )
    .max(0.0)
}

/// Resolves every item's flex base size
/// (<https://drafts.csswg.org/css-flexbox-1/#flex-base-size>).
fn resolve_items<'a>(b: &'a BoxNode, ctx: &Ctx<'_>, container: &Container) -> Vec<Item<'a>> {
    let mut children: Vec<&BoxNode> = b.children.iter().collect();
    if container.reverse {
        children.reverse();
    }
    let mut items = Vec::with_capacity(children.len());
    for node in children {
        let node_style = node.style;
        let root = ctx.root_font_size;
        let margin = node_style.margin.map(|dimension| match dimension {
            Dimension::Auto => 0.0,
            Dimension::Length(length) => {
                length.resolve(container.content_width, node_style.font_size, root)
            }
        });
        let padding = node_style
            .padding
            .map(|length| length.resolve(container.content_width, node_style.font_size, root));
        let border = Edges::new(
            node_style.border.top.width,
            node_style.border.right.width,
            node_style.border.bottom.width,
            node_style.border.left.width,
        );
        let extras_main = if container.row {
            padding.horizontal() + border.horizontal()
        } else {
            padding.vertical() + border.vertical()
        };
        let extras_cross = if container.row {
            padding.vertical() + border.vertical()
        } else {
            padding.horizontal() + border.horizontal()
        };
        let base = flex_base_size(node, ctx, container, extras_main);
        items.push(Item {
            node,
            margin,
            extras_main,
            extras_cross,
            base,
            main: base,
            grow: node_style.flex_grow,
            shrink: node_style.flex_shrink,
        });
    }
    items
}

/// The flex base size of one item (9.2).
fn flex_base_size(node: &BoxNode, ctx: &Ctx<'_>, container: &Container, extras_main: f32) -> f32 {
    let style = node.style;
    let root = ctx.root_font_size;
    if let Dimension::Length(length) = style.flex_basis {
        return length
            .resolve(container.main_size, style.font_size, root)
            .max(0.0);
    }
    let specified = if container.row {
        style.width
    } else {
        style.height
    };
    if let Dimension::Length(length) = specified {
        let value = length.resolve(container.main_size, style.font_size, root);
        return if style.box_sizing == BoxSizing::BorderBox {
            (value - extras_main).max(0.0)
        } else {
            value.max(0.0)
        };
    }
    if container.row {
        (max_content_width(node, ctx) - extras_main).max(0.0)
    } else {
        let layout = layout_block_sized(
            node,
            ctx,
            Containing {
                width: container.content_width,
                height: None,
                x: 0.0,
                y: 0.0,
            },
            None,
            None,
        );
        layout.content_box().height.max(0.0)
    }
}

/// Greedy line breaking (9.3); `nowrap` keeps one line.
fn break_lines(
    items: &[Item<'_>],
    container: &Container,
    gap: f32,
    wraps: bool,
) -> Vec<Vec<usize>> {
    if !wraps {
        return vec![(0..items.len()).collect()];
    }
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut used = 0.0_f32;
    for (index, item) in items.iter().enumerate() {
        let outer = outer_main(item, container.row);
        if !current.is_empty() && used + gap + outer > container.main_size {
            lines.push(std::mem::take(&mut current));
            used = 0.0;
        }
        if !current.is_empty() {
            used += gap;
        }
        used += outer;
        current.push(index);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Lays out every line and returns the painted items and total cross size.
fn place_lines(
    b: &BoxNode,
    ctx: &Ctx<'_>,
    container: &Container,
    items: &[Item<'_>],
    lines: &[Vec<usize>],
    gap: f32,
) -> (Vec<PaintItem>, f32) {
    let style = b.style;
    let mut painted: Vec<PaintItem> = Vec::new();
    let mut y = container.content_y;
    let mut total_cross = 0.0_f32;
    for (line_index, line) in lines.iter().enumerate() {
        let cross = line_cross(items, line, ctx);
        let leftover = main_leftover(items, line, container, gap);
        let (mut offset, between) = justify(style.justify_content, leftover, line.len(), gap);
        if container.reverse {
            offset = leftover - offset;
        }
        let mut x = container.content_x;
        if container.row {
            x += offset.max(0.0);
        } else {
            y += offset.max(0.0);
        }
        for &index in line {
            let item = &items[index];
            let align = align_self(item.node.style.align_self, style.align_items);
            let item_cross = if is_stretch(align) && container.cross_auto {
                (container.cross_size - item.extras_cross - cross_margins(item, container.row))
                    .max(0.0)
            } else {
                -1.0
            };
            let containing = if container.row {
                Containing {
                    width: container.main_size,
                    height: Some(container.cross_size),
                    x,
                    y,
                }
            } else {
                Containing {
                    width: container.cross_size,
                    height: Some(container.main_size),
                    x,
                    y,
                }
            };
            let layout = layout_block_sized(
                item.node,
                ctx,
                containing,
                Some(item.main),
                if item_cross >= 0.0 {
                    Some(item_cross)
                } else {
                    None
                },
            );
            let outer_cross = cross_size(&layout, item, container.row);
            let cross_offset = match align {
                AlignItems::FlexStart | AlignItems::Baseline | AlignItems::Stretch => 0.0,
                AlignItems::Center => (cross - outer_cross).max(0.0) / 2.0,
                AlignItems::FlexEnd => (cross - outer_cross).max(0.0),
            };
            let mut layout = layout;
            if container.row {
                layout.rect.y += cross_offset;
                move_children(&mut layout, 0.0, cross_offset);
            } else {
                layout.rect.x += cross_offset;
                move_children(&mut layout, cross_offset, 0.0);
            }
            painted.push(PaintItem::Box(Box::new(layout)));
            if container.row {
                x += outer_main(item, true) + between;
            } else {
                y += outer_main(item, false) + between;
            }
        }
        total_cross += cross;
        if line_index + 1 < lines.len() {
            total_cross += gap;
        }
        if container.row {
            y += cross + gap;
        }
    }
    (painted, total_cross)
}

/// Shifts a laid-out subtree, used for align/justify offsets.
fn move_children(layout: &mut LayoutBox, dx: f32, dy: f32) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    for item in &mut layout.items {
        if let PaintItem::Box(child) = item {
            child.rect.x += dx;
            child.rect.y += dy;
            move_children(child, dx, dy);
        }
    }
}

/// Resolves grow/shrink for one line (9.7).
fn resolve_line(items: &mut [Item<'_>], line: &[usize], container: &Container, gap: f32) {
    let used: f32 = line
        .iter()
        .map(|&index| outer_main(&items[index], container.row))
        .sum::<f32>()
        + gap * crate::count(line.len().saturating_sub(1));
    let free = container.main_size - used;
    if free > 0.0 {
        let total_grow: f32 = line.iter().map(|&index| items[index].grow).sum();
        if total_grow > 0.0 {
            for &index in line {
                let item = &mut items[index];
                item.main = item.base + free * item.grow / total_grow;
            }
        }
    } else if free < 0.0 {
        let total_weight: f32 = line
            .iter()
            .map(|&index| {
                let item = &items[index];
                item.shrink * item.base
            })
            .sum();
        if total_weight > 0.0 {
            for &index in line {
                let item = &mut items[index];
                let share = free * (item.shrink * item.base) / total_weight;
                item.main = (item.base + share).max(0.0);
            }
        }
    }
}

/// Outer main size of one item.
fn outer_main(item: &Item<'_>, row: bool) -> f32 {
    item.main
        + item.extras_main
        + if row {
            item.margin.left + item.margin.right
        } else {
            item.margin.top + item.margin.bottom
        }
}

/// The item's cross-axis margins.
fn cross_margins(item: &Item<'_>, row: bool) -> f32 {
    if row {
        item.margin.top + item.margin.bottom
    } else {
        item.margin.left + item.margin.right
    }
}

/// The laid-out cross size of one item including margins.
fn cross_size(layout: &LayoutBox, item: &Item<'_>, row: bool) -> f32 {
    if row {
        layout.rect.height + item.margin.top + item.margin.bottom
    } else {
        layout.rect.width + item.margin.left + item.margin.right
    }
}

/// Remaining main-axis space after items and gaps.
fn main_leftover(items: &[Item<'_>], line: &[usize], container: &Container, gap: f32) -> f32 {
    let used: f32 = line
        .iter()
        .map(|&index| outer_main(&items[index], container.row))
        .sum::<f32>()
        + gap * crate::count(line.len().saturating_sub(1));
    (container.main_size - used).max(0.0)
}
/// The cross size of one line: the tallest item.
fn line_cross(items: &[Item<'_>], line: &[usize], ctx: &Ctx<'_>) -> f32 {
    line.iter()
        .map(|&index| {
            let item = &items[index];
            let layout = layout_block_sized(
                item.node,
                ctx,
                Containing {
                    width: item.main,
                    height: None,
                    x: 0.0,
                    y: 0.0,
                },
                Some(item.main),
                None,
            );
            layout.rect.height + item.margin.top + item.margin.bottom
        })
        .fold(0.0, f32::max)
}

/// Main-axis distribution from `justify-content`.
fn justify(justify: JustifyContent, leftover: f32, count: usize, gap: f32) -> (f32, f32) {
    if count == 0 {
        return (0.0, gap);
    }
    match justify {
        JustifyContent::FlexStart => (0.0, gap),
        JustifyContent::FlexEnd => (leftover, gap),
        JustifyContent::Center => (leftover / 2.0, gap),
        JustifyContent::SpaceBetween => {
            if count > 1 {
                (0.0, gap + leftover / crate::count(count - 1))
            } else {
                (0.0, gap)
            }
        }
        JustifyContent::SpaceAround => {
            let share = leftover / crate::count(count);
            (share / 2.0, gap + share)
        }
        JustifyContent::SpaceEvenly => {
            let share = leftover / crate::count(count + 1);
            (share, gap + share)
        }
    }
}

/// Resolves `align-self` against the container default.
fn align_self(value: AlignSelf, default: AlignItems) -> AlignItems {
    match value {
        AlignSelf::Auto => default,
        AlignSelf::Stretch => AlignItems::Stretch,
        AlignSelf::FlexStart => AlignItems::FlexStart,
        AlignSelf::FlexEnd => AlignItems::FlexEnd,
        AlignSelf::Center => AlignItems::Center,
        AlignSelf::Baseline => AlignItems::Baseline,
    }
}

/// Whether an alignment stretches the item.
fn is_stretch(align: AlignItems) -> bool {
    align == AlignItems::Stretch
}
