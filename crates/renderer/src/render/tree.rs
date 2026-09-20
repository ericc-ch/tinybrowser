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

use crate::render::RasterImage;
use crate::render::geometry::Edges;
use crate::render::style::{BorderSide, Dimension, Display, Length, Style};
use crate::render::svg;

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
pub(crate) fn build(
    dom: &Dom,
    styles: &HashMap<NodeId, Style>,
    images: &HashMap<NodeId, RasterImage>,
) -> BoxNode {
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
    let children = build_children(dom, styles, images, document, &root_style);
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
    images: &HashMap<NodeId, RasterImage>,
    parent: NodeId,
    parent_style: &Style,
) -> Vec<BoxNode> {
    let children = dom.rendered_children(parent);
    let mut boxes = Vec::new();
    for child in children {
        match dom.kind(child) {
            Some(dom::NodeKind::Element { name, .. }) => {
                let mut style = styles
                    .get(&child)
                    .cloned()
                    .unwrap_or_else(|| Style::inherited_from(parent_style));
                // `opacity` forms a group for the complete descendant paint
                // subtree. Carry the effective value through inline boxes,
                // which our layout flattens away.
                // https://drafts.csswg.org/css-color-4/#transparency
                style.opacity *= parent_style.opacity;
                if let Some(image) = images.get(&child) {
                    apply_image_dimensions(dom, child, image, &mut style);
                } else if svg::is_outer_svg(dom, child) {
                    svg::apply_dimensions(dom, child, &mut style);
                }
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
                let children = if is_break || svg::is_outer_svg(dom, child) {
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
                    let kids = build_children(dom, styles, images, child, &style);
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
                boxes.extend(build_children(dom, styles, images, child, parent_style));
            }
            _ => {}
        }
    }
    boxes
}

/// Applies a decoded image's natural dimensions and aspect ratio to its
/// replaced box. The HTML `width` and `height` attributes are presentational
/// hints; authored CSS wins because it has already cascaded into non-auto
/// dimensions
/// (<https://html.spec.whatwg.org/multipage/rendering.html#attributes-for-embedded-content-and-images>,
/// <https://drafts.csswg.org/css-images-3/#sizing>).
fn apply_image_dimensions(dom: &Dom, id: NodeId, image: &RasterImage, style: &mut Style) {
    if image.width == 0 || image.height == 0 {
        return;
    }
    style.aspect_ratio =
        Some(crate::render::pixels(image.width) / crate::render::pixels(image.height));

    if style.width == Dimension::Auto {
        style.width = image_dimension_attribute(dom, id, "width").unwrap_or(Dimension::Auto);
    }
    if style.height == Dimension::Auto {
        style.height = image_dimension_attribute(dom, id, "height").unwrap_or(Dimension::Auto);
    }
    if style.width == Dimension::Auto && style.height == Dimension::Auto {
        style.width = Dimension::Length(Length::Px(crate::render::pixels(image.width)));
    }
}

/// Parses the non-negative integer or legacy percentage syntax accepted by
/// image dimension presentational hints.
fn image_dimension_attribute(dom: &Dom, id: NodeId, name: &str) -> Option<Dimension> {
    let value = dom.attribute(id, name)?;
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        let number = percent.trim().parse::<f32>().ok()?;
        return (number.is_finite() && number >= 0.0)
            .then_some(Dimension::Length(Length::Percent(number)));
    }
    let number = value.parse::<u32>().ok()?;
    Some(Dimension::Length(Length::Px(crate::render::pixels(number))))
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
        // https://html.spec.whatwg.org/multipage/rendering.html#the-input-element-as-a-text-entry-widget
        "password" => Some(
            dom.input_value(id)
                .map(|value| "\u{2022}".repeat(value.chars().count()))
                .unwrap_or_default(),
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
