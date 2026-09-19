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

use crate::geometry::Edges;
use crate::style::{BorderSide, Display, Style};

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
    /// Block-level grid container.
    Grid,
    /// Inline-level grid container.
    InlineGrid,
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
    /// The DOM node this box was generated for, when it is an element box.
    pub(crate) node: Option<NodeId>,
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
        border: Edges::new(
            BorderSide::NONE,
            BorderSide::NONE,
            BorderSide::NONE,
            BorderSide::NONE,
        ),
        ..Style::initial()
    };
    let children = build_children(dom, styles, document, &root_style);
    BoxNode {
        kind: BoxKind::Block,
        node: None,
        style: root_style.clone(),
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
    let children = dom.rendered_children(parent);
    let mut boxes = Vec::new();
    for child in children {
        match dom.kind(child) {
            Some(dom::NodeKind::Element { name, .. }) => {
                let style = styles
                    .get(&child)
                    .cloned()
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
                        Display::Grid => BoxKind::Grid,
                        Display::InlineGrid => BoxKind::InlineGrid,
                        Display::ListItem => BoxKind::ListItem,
                        Display::None => continue,
                    }
                };
                let children = if is_break {
                    Vec::new()
                } else if let Some(value) = rendered_input_text(dom, child) {
                    if value.is_empty() {
                        Vec::new()
                    } else {
                        vec![BoxNode {
                            kind: BoxKind::Text(value),
                            node: None,
                            style: style.clone(),
                            children: Vec::new(),
                        }]
                    }
                } else {
                    let kids = build_children(dom, styles, child, &style);
                    wrap_anonymous(kids, &style)
                };
                boxes.push(BoxNode {
                    kind,
                    node: Some(child),
                    style,
                    children,
                });
            }
            Some(dom::NodeKind::Text { data } | dom::NodeKind::CDataSection { data }) => {
                if data.is_empty() {
                    continue;
                }
                boxes.push(BoxNode {
                    kind: BoxKind::Text(data.clone()),
                    node: None,
                    style: parent_style.clone(),
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

/// Text painted inside the UA widget for the input states whose value is
/// textual. Form-control appearance is UA-defined; the value itself comes
/// from HTML's live value state
/// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
fn rendered_input_text(dom: &Dom, id: NodeId) -> Option<String> {
    let dom::NodeKind::Element { name, .. } = dom.kind(id)? else {
        return None;
    };
    if name.ns != dom::html_namespace() || name.local.as_ref() != "input" {
        return None;
    }
    let input_type = dom.input_type(id).unwrap_or_else(|| "text".into());
    match input_type.as_str() {
        "hidden" | "checkbox" | "radio" | "file" | "image" | "range" | "color" => {
            Some(String::new())
        }
        "submit" => Some(
            dom.input_value(id)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Submit".into()),
        ),
        "reset" => Some(
            dom.input_value(id)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Reset".into()),
        ),
        _ => dom.input_value(id),
    }
}

/// Whether a box participates in block flow.
pub(crate) fn is_block_level(node: &BoxNode) -> bool {
    matches!(
        node.kind,
        BoxKind::Block | BoxKind::Flex | BoxKind::Grid | BoxKind::ListItem
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
    // Anonymous boxes inherit inherited properties (font, color, alignment)
    // and start with initial values for everything else. `text-decoration` is
    // not an inherited property, but it propagates into anonymous boxes from
    // the box that established it
    // (<https://drafts.csswg.org/css-text-decor-3/#line-decoration>).
    let anonymous_style = Style {
        display: Display::Block,
        text_decoration: parent_style.text_decoration,
        border: Edges::new(
            BorderSide::NONE,
            BorderSide::NONE,
            BorderSide::NONE,
            BorderSide::NONE,
        ),
        ..Style::inherited_from(parent_style)
    };

    let mut out = Vec::with_capacity(children.len());
    let mut pending: Vec<BoxNode> = Vec::new();
    for child in children {
        if is_block_level(&child) {
            push_anonymous(&mut out, &anonymous_style, &mut pending);
            out.push(child);
        } else {
            pending.push(child);
        }
    }
    push_anonymous(&mut out, &anonymous_style, &mut pending);
    out
}

/// Pushes one pending inline run as an anonymous block, unless it holds
/// nothing that could paint: a run of collapsed whitespace between block-level
/// boxes generates no box at all
/// (<https://drafts.csswg.org/css-display-3/#anonymous>), and wrapping it
/// would break margin collapsing between its neighbors.
fn push_anonymous(out: &mut Vec<BoxNode>, style: &Style, pending: &mut Vec<BoxNode>) {
    if pending.is_empty() {
        return;
    }
    if pending.iter().any(could_paint) {
        out.push(BoxNode {
            kind: BoxKind::Block,
            node: None,
            style: style.clone(),
            children: std::mem::take(pending),
        });
    } else {
        pending.clear();
    }
}

/// Whether a box could paint anything: text with visible characters, or any
/// inline element, atomic, or break (their own box may paint).
fn could_paint(node: &BoxNode) -> bool {
    match &node.kind {
        BoxKind::Text(text) => text
            .chars()
            .any(|ch| !matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}')),
        _ => true,
    }
}
