//! Box tree construction: DOM + computed styles -> boxes.
//!
//! Follows CSS Display's box generation
//! (<https://drafts.csswg.org/css-display-3/#box-generation>): `display:
//! none` prunes a subtree, block containers that mix block-level and
//! inline-level children get anonymous block boxes around runs of inline
//! content, and text nodes become text boxes that inherit their parent's
//! style.

use std::collections::HashMap;

use dom::{Dom, NodeId};

use crate::color::Color;
use crate::geometry::Edges;
use crate::style::{BorderSide, BorderStyle, Dimension, Display, Length, Style};

/// What kind of box a node generated.
pub(crate) enum BoxKind {
    /// Block-level block container.
    Block,
    /// Inline-level box that participates in a line.
    Inline,
    /// Inline-level block container.
    InlineBlock,
    /// Block-level flex container.
    Flex,
    /// Inline-level flex container.
    InlineFlex,
    /// Block-level list item.
    ListItem,
    /// A text box.
    Text(String),
    /// `<br>`.
    Break,
}

/// One box in the tree.
pub(crate) struct BoxNode {
    /// Box kind.
    pub(crate) kind: BoxKind,
    /// Computed style (for text boxes, the parent element's style).
    pub(crate) style: Style,
    /// Children in tree order.
    pub(crate) children: Vec<BoxNode>,
}

/// Builds the box tree for one document.
pub(crate) fn build(dom: &Dom, styles: &HashMap<NodeId, Style>) -> BoxNode {
    let document = dom.document();
    let root_style = Style {
        display: Display::Block,
        ..Style::INITIAL
    };
    let children = build_children(dom, styles, document, &root_style);
    BoxNode {
        kind: BoxKind::Block,
        style: root_style,
        children: wrap_anonymous(children, &root_style),
    }
}

/// Builds the boxes for every child of `parent`.
fn build_children(
    dom: &Dom,
    styles: &HashMap<NodeId, Style>,
    parent: NodeId,
    parent_style: &Style,
) -> Vec<BoxNode> {
    let Some(children) = dom.children(parent) else {
        return Vec::new();
    };
    let mut boxes = Vec::new();
    for &child in children {
        match dom.kind(child) {
            Some(dom::NodeKind::Element { name, .. }) => {
                let style = styles
                    .get(&child)
                    .copied()
                    .unwrap_or_else(|| Style::inherited_from(parent_style));
                if style.display == Display::None {
                    continue;
                }
                let is_break = name.ns == dom::html_namespace() && name.local.as_ref() == "br";
                let kind = if is_break {
                    BoxKind::Break
                } else {
                    match style.display {
                        Display::Block => BoxKind::Block,
                        Display::Inline => BoxKind::Inline,
                        Display::InlineBlock => BoxKind::InlineBlock,
                        Display::Flex => BoxKind::Flex,
                        Display::InlineFlex => BoxKind::InlineFlex,
                        Display::ListItem => BoxKind::ListItem,
                        Display::None => continue,
                    }
                };
                let children = if is_break {
                    Vec::new()
                } else {
                    let kids = build_children(dom, styles, child, &style);
                    wrap_anonymous(kids, &style)
                };
                boxes.push(BoxNode {
                    kind,
                    style,
                    children,
                });
            }
            Some(dom::NodeKind::Text { data } | dom::NodeKind::CDataSection { data }) => {
                if data.is_empty() || (collapses(parent_style) && is_all_collapsible(data)) {
                    continue;
                }
                boxes.push(BoxNode {
                    kind: BoxKind::Text(data.clone()),
                    style: *parent_style,
                    children: Vec::new(),
                });
            }
            Some(dom::NodeKind::Document | dom::NodeKind::Fragment) => {
                boxes.extend(build_children(dom, styles, child, parent_style));
            }
            _ => {}
        }
    }
    boxes
}

/// Whether a style collapses whitespace.
fn collapses(style: &Style) -> bool {
    matches!(
        style.white_space,
        crate::style::WhiteSpace::Normal | crate::style::WhiteSpace::Nowrap
    )
}

/// Whether text is entirely collapsible whitespace.
fn is_all_collapsible(text: &str) -> bool {
    text.chars()
        .all(|ch| matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}'))
}

/// Whether a box participates in block flow.
pub(crate) fn is_block_level(node: &BoxNode) -> bool {
    matches!(
        node.kind,
        BoxKind::Block | BoxKind::Flex | BoxKind::ListItem
    )
}

/// Whether a box is inline-level.
fn is_inline_level(node: &BoxNode) -> bool {
    !is_block_level(node)
}

/// Wraps runs of inline-level children in anonymous block boxes when a block
/// container has both kinds of children
/// (<https://drafts.csswg.org/css-display-3/#anonymous>).
fn wrap_anonymous(children: Vec<BoxNode>, parent_style: &Style) -> Vec<BoxNode> {
    let has_block = children.iter().any(is_block_level);
    let has_inline = children.iter().any(is_inline_level);
    if !has_block || !has_inline {
        return children;
    }
    let anonymous_style = Style {
        display: Display::Block,
        margin: Edges::new(
            Dimension::Length(Length::Px(0.0)),
            Dimension::Length(Length::Px(0.0)),
            Dimension::Length(Length::Px(0.0)),
            Dimension::Length(Length::Px(0.0)),
        ),
        padding: Edges::new(
            Length::Px(0.0),
            Length::Px(0.0),
            Length::Px(0.0),
            Length::Px(0.0),
        ),
        border: Edges::new(
            BorderSide {
                width: 0.0,
                style: BorderStyle::None,
                color: Color::BLACK,
            },
            BorderSide {
                width: 0.0,
                style: BorderStyle::None,
                color: Color::BLACK,
            },
            BorderSide {
                width: 0.0,
                style: BorderStyle::None,
                color: Color::BLACK,
            },
            BorderSide {
                width: 0.0,
                style: BorderStyle::None,
                color: Color::BLACK,
            },
        ),
        background: Color::TRANSPARENT,
        ..*parent_style
    };

    let mut out = Vec::with_capacity(children.len());
    let mut pending: Vec<BoxNode> = Vec::new();
    for child in children {
        if is_block_level(&child) {
            if !pending.is_empty() {
                out.push(BoxNode {
                    kind: BoxKind::Block,
                    style: anonymous_style,
                    children: std::mem::take(&mut pending),
                });
            }
            out.push(child);
        } else {
            pending.push(child);
        }
    }
    if !pending.is_empty() {
        out.push(BoxNode {
            kind: BoxKind::Block,
            style: anonymous_style,
            children: pending,
        });
    }
    out
}
