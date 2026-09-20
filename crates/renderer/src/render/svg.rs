//! Minimal inline SVG replaced sizing and shape paint.
//!
//! SVG stays in the DOM for selectors and script. The outer `<svg>` generates
//! one replaced CSS box; its path and rectangle descendants paint in the SVG
//! viewport rather than participating in CSS box layout.

use std::collections::HashMap;

use dom::{Dom, NodeId};
use kurbo::{BezPath, PathEl};
use tiny_skia::{PathBuilder, Transform};

use crate::render::RasterImage;
use crate::render::color::Color;
use crate::render::geometry::Rect;
use crate::render::paint::Painter;
use crate::render::style::{Dimension, Display, Length, Style, Visibility};

/// Whether `node` is an outer SVG viewport element.
pub(crate) fn is_outer_svg(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.kind(node),
        Some(dom::NodeKind::Element { name, .. })
            if name.ns == dom::svg_namespace() && name.local.as_ref() == "svg"
    )
}

/// Applies outer SVG intrinsic dimensions to its replaced CSS box
/// (<https://svgwg.org/svg2-draft/coords.html#SizingSVGInCSS>).
pub(crate) fn apply_dimensions(dom: &Dom, node: NodeId, style: &mut Style) {
    let viewport = view_box(dom, node);
    let width = positive_length(dom.attribute(node, "width").as_deref());
    let height = positive_length(dom.attribute(node, "height").as_deref());
    let ratio = viewport
        .filter(|viewport| viewport.width > 0.0 && viewport.height > 0.0)
        .map(|viewport| viewport.width / viewport.height)
        .or_else(|| width.zip(height).map(|(width, height)| width / height));
    style.aspect_ratio = ratio;

    if style.width == Dimension::Auto {
        style.width = width.map_or(Dimension::Auto, |value| {
            Dimension::Length(Length::Px(value))
        });
    }
    if style.height == Dimension::Auto {
        style.height = height.map_or(Dimension::Auto, |value| {
            Dimension::Length(Length::Px(value))
        });
    }
}

/// Rasterizes one outer SVG into a transparent bitmap for `<img src>`.
///
/// Script does not run. Missing width/height fall back to the viewBox, then
/// to the CSS replaced-element default of 300×150
/// (<https://svgwg.org/svg2-draft/coords.html#SizingSVGInCSS>,
/// <https://drafts.csswg.org/css-images-3/#default-sizing>).
pub(crate) fn rasterize(dom: &Dom, root: NodeId) -> Option<RasterImage> {
    let mut style = Style::initial();
    apply_dimensions(dom, root, &mut style);
    let viewport = view_box(dom, root);
    let width = dimension_px(style.width)
        .or_else(|| viewport.map(|viewport| viewport.width))
        .filter(|value| *value > 0.0)
        .unwrap_or(300.0);
    let height = dimension_px(style.height)
        .or_else(|| viewport.map(|viewport| viewport.height))
        .filter(|value| *value > 0.0)
        .unwrap_or(150.0);
    let width_px = device_side(width)?;
    let height_px = device_side(height)?;
    if !crate::render::decoded_rgba_fits(width_px, height_px) {
        return None;
    }
    let mut painter =
        Painter::with_background(width_px, height_px, tiny_skia::Color::TRANSPARENT).ok()?;
    paint(
        &mut painter,
        dom,
        &HashMap::new(),
        root,
        Rect::new(
            0.0,
            0.0,
            crate::render::pixels(width_px),
            crate::render::pixels(height_px),
        ),
    );
    let image = painter.into_image();
    Some(RasterImage {
        width: image.width,
        height: image.height,
        data: image.data,
    })
}

fn dimension_px(value: Dimension) -> Option<f32> {
    match value {
        Dimension::Length(Length::Px(value)) if value > 0.0 => Some(value),
        _ => None,
    }
}

fn device_side(value: f32) -> Option<u32> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let rounded = value.round();
    if rounded > f32::from(crate::render::MAX_DECODED_SIDE) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "SVG intrinsic size is clamped to a bounded pixel side"
    )]
    let side = rounded as u32;
    (side > 0).then_some(side)
}

/// Paints supported SVG geometry into the outer SVG content box
/// (<https://svgwg.org/svg2-draft/render.html#RenderingOrder>).
pub(crate) fn paint(
    painter: &mut Painter,
    dom: &Dom,
    styles: &HashMap<NodeId, Style>,
    root: NodeId,
    destination: Rect,
) {
    let viewport = view_box(dom, root).unwrap_or(ViewBox {
        x: 0.0,
        y: 0.0,
        width: destination.width,
        height: destination.height,
    });
    if viewport.width <= 0.0 || viewport.height <= 0.0 || destination.is_empty() {
        return;
    }
    let scale = (destination.width / viewport.width).min(destination.height / viewport.height);
    let transform = Transform::from_row(
        scale,
        0.0,
        0.0,
        scale,
        destination.x + (destination.width - viewport.width * scale) / 2.0 - viewport.x * scale,
        destination.y + (destination.height - viewport.height * scale) / 2.0 - viewport.y * scale,
    );
    let inherited = styles.get(&root).map_or(Color::BLACK, |style| style.color);
    paint_children(painter, dom, styles, root, viewport, transform, inherited);
}

fn paint_children(
    painter: &mut Painter,
    dom: &Dom,
    styles: &HashMap<NodeId, Style>,
    parent: NodeId,
    viewport: ViewBox,
    transform: Transform,
    inherited: Color,
) {
    let Some(children) = dom.children(parent) else {
        return;
    };
    for child in children.clone().copied() {
        let Some(dom::NodeKind::Element { name, .. }) = dom.kind(child) else {
            continue;
        };
        if name.ns != dom::svg_namespace() {
            continue;
        }
        let style = styles.get(&child);
        if style.is_some_and(|style| {
            style.visibility != Visibility::Visible
                || style.display == Display::None
                || style.opacity <= 0.0
        }) {
            continue;
        }
        let color = fill_color(dom.attribute(child, "fill").as_deref(), style, inherited);
        match name.local.as_ref() {
            "path" => {
                if let Some(path) = dom.attribute(child, "d").and_then(|data| path(&data)) {
                    painter.fill_vector_path(&path, color, transform);
                }
            }
            "rect" => {
                if let Some(path) = rectangle(dom, child, viewport) {
                    painter.fill_vector_path(&path, color, transform);
                }
            }
            "g" | "a" | "svg" => {
                paint_children(painter, dom, styles, child, viewport, transform, color);
            }
            // Definition and foreign-content subtrees do not paint directly.
            "clipPath" | "defs" | "filter" | "foreignObject" | "metadata" | "title" => {}
            _ => paint_children(painter, dom, styles, child, viewport, transform, color),
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "SVG path coordinates paint into a bounded f32 viewport"
)]
fn path(data: &str) -> Option<tiny_skia::Path> {
    let path = BezPath::from_svg(data).ok()?;
    let mut builder = PathBuilder::new();
    for element in path.elements() {
        match *element {
            PathEl::MoveTo(point) => builder.move_to(point.x as f32, point.y as f32),
            PathEl::LineTo(point) => builder.line_to(point.x as f32, point.y as f32),
            PathEl::QuadTo(control, point) => builder.quad_to(
                control.x as f32,
                control.y as f32,
                point.x as f32,
                point.y as f32,
            ),
            PathEl::CurveTo(first, second, point) => builder.cubic_to(
                first.x as f32,
                first.y as f32,
                second.x as f32,
                second.y as f32,
                point.x as f32,
                point.y as f32,
            ),
            PathEl::ClosePath => builder.close(),
        }
    }
    builder.finish()
}

fn rectangle(dom: &Dom, node: NodeId, viewport: ViewBox) -> Option<tiny_skia::Path> {
    let x = coordinate(dom.attribute(node, "x").as_deref(), viewport.width).unwrap_or(0.0);
    let y = coordinate(dom.attribute(node, "y").as_deref(), viewport.height).unwrap_or(0.0);
    let width = coordinate(dom.attribute(node, "width").as_deref(), viewport.width)?;
    let height = coordinate(dom.attribute(node, "height").as_deref(), viewport.height)?;
    let rect = tiny_skia::Rect::from_xywh(x, y, width, height)?;
    Some(PathBuilder::from_rect(rect))
}

fn fill_color(value: Option<&str>, style: Option<&Style>, inherited: Color) -> Color {
    let value = value.map(str::trim);
    match value {
        Some("none") => Color::TRANSPARENT,
        Some("currentColor") => style.map_or(inherited, |style| style.color),
        Some(value) => parse_color(value).unwrap_or(inherited),
        None => inherited,
    }
}

fn parse_color(value: &str) -> Option<Color> {
    if let Some(hex) = value.strip_prefix('#')
        && hex.len() == 6
    {
        return Some(Color::rgb(
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        ));
    }
    match value.to_ascii_lowercase().as_str() {
        "black" => Some(Color::BLACK),
        "blue" => Some(Color::rgb(0, 0, 255)),
        "green" => Some(Color::rgb(0, 128, 0)),
        "lime" => Some(Color::rgb(0, 255, 0)),
        "red" => Some(Color::rgb(255, 0, 0)),
        "white" => Some(Color::rgb(255, 255, 255)),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct ViewBox {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

fn view_box(dom: &Dom, node: NodeId) -> Option<ViewBox> {
    let value = dom.attribute(node, "viewBox")?;
    let mut values = value
        .split(|character: char| character == ',' || character.is_ascii_whitespace())
        .filter(|part| !part.is_empty())
        .map(str::parse::<f32>);
    let viewport = ViewBox {
        x: values.next()?.ok()?,
        y: values.next()?.ok()?,
        width: values.next()?.ok()?,
        height: values.next()?.ok()?,
    };
    values.next().is_none().then_some(viewport)
}

fn positive_length(value: Option<&str>) -> Option<f32> {
    let value = value?.trim();
    let value = value.strip_suffix("px").unwrap_or(value);
    let number = value.parse::<f32>().ok()?;
    (number.is_finite() && number >= 0.0).then_some(number)
}

fn coordinate(value: Option<&str>, basis: f32) -> Option<f32> {
    let value = value?.trim();
    if let Some(percent) = value.strip_suffix('%') {
        return percent
            .parse::<f32>()
            .ok()
            .map(|number| basis * number / 100.0);
    }
    value
        .strip_suffix("px")
        .unwrap_or(value)
        .parse::<f32>()
        .ok()
}
