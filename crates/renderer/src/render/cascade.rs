//! The render pipeline: cascade styles over the DOM, build the box tree, lay
//! it out, and paint one image.

use dom::Dom;

use crate::render::color::Color;
use crate::render::font::Fonts;
use crate::render::geometry::Rect;
use crate::render::layout::{LayoutBox, PaintItem};
use crate::render::paint::Painter;
use crate::render::stylo::style_document;
use crate::render::tree;
use crate::render::{RasterImage, RenderError, RenderOptions, RgbaImage};

/// Renders `dom` into one image.
///
/// # Errors
///
/// See [`RenderError`].
pub(crate) fn render(
    dom: &Dom,
    stylesheets: &[String],
    options: &RenderOptions,
    images: &std::collections::HashMap<dom::NodeId, RasterImage>,
) -> Result<RgbaImage, RenderError> {
    if !valid_dimension(options.width)
        || !valid_dimension(options.height)
        || !valid_dimension(options.scale)
    {
        return Err(RenderError::InvalidViewport);
    }
    let viewport_width = options.width;
    let viewport_height = options.height;

    // 1. Style every element through Stylo: UA sheet, author sheets in
    // document order, and `style` attributes.
    // 2. Compute one style per element in tree order.
    let fonts = Fonts::load()?;
    let mut styles = style_document(dom, stylesheets, viewport_width, viewport_height);
    let canvas_background = propagate_canvas_background(dom, &mut styles);

    // 3. Build and lay out the box tree through Taffy.
    let root = tree::build(dom, &styles, images);
    let layout = crate::render::boxes::layout_root(&root, &fonts, viewport_width, viewport_height);

    // 4. Paint once, then encode from the caller.
    let width = crate::render::device_pixels((viewport_width * options.scale).round().max(1.0));
    let height = crate::render::device_pixels((viewport_height * options.scale).round().max(1.0));
    let mut painter = Painter::new(width, height)?;
    painter.fill_rect(
        Rect::new(0.0, 0.0, options.width, options.height),
        canvas_background,
    );
    paint(&mut painter, &layout, &fonts, images, dom, &styles);
    Ok(painter.into_image())
}

/// Moves the HTML root/body background color onto the document canvas.
///
/// The source element's used background becomes transparent after
/// propagation, so translucent colors are blended into the canvas only once
/// (<https://drafts.csswg.org/css-backgrounds-3/#special-backgrounds>).
fn propagate_canvas_background(
    dom: &Dom,
    styles: &mut std::collections::HashMap<dom::NodeId, crate::render::style::Style>,
) -> Color {
    let Some(root) = dom.select_first(dom.document(), "html").ok().flatten() else {
        return Color::TRANSPARENT;
    };
    let Some(root_style) = styles.get(&root) else {
        return Color::TRANSPARENT;
    };
    if root_style.display == crate::render::style::Display::None {
        return Color::TRANSPARENT;
    }

    let source = if root_style.background.a > 0 {
        root
    } else {
        dom.children(root)
            .and_then(|children| {
                children.clone().copied().find(|child| {
                    matches!(
                        dom.kind(*child),
                        Some(dom::NodeKind::Element { name, .. })
                            if name.ns == dom::html_namespace() && name.local.as_ref() == "body"
                    )
                })
            })
            .unwrap_or(root)
    };
    let Some(source_style) = styles.get_mut(&source) else {
        return Color::TRANSPARENT;
    };
    if source_style.display == crate::render::style::Display::None {
        return Color::TRANSPARENT;
    }
    std::mem::replace(&mut source_style.background, Color::TRANSPARENT)
}

/// Lays `dom` out without painting and returns every box in tree order, for
/// script geometry (`getBoundingClientRect`, hit testing).
pub(crate) fn boxes(
    dom: &Dom,
    stylesheets: &[String],
    options: &RenderOptions,
    images: &std::collections::HashMap<dom::NodeId, RasterImage>,
) -> Result<Vec<crate::render::NodeBox>, RenderError> {
    if !valid_dimension(options.width)
        || !valid_dimension(options.height)
        || !valid_dimension(options.scale)
    {
        return Err(RenderError::InvalidViewport);
    }
    let fonts = Fonts::load()?;
    let styles = style_document(dom, stylesheets, options.width, options.height);
    let root = tree::build(dom, &styles, images);
    let layout = crate::render::boxes::layout_root(&root, &fonts, options.width, options.height);
    let mut out = Vec::new();
    collect_boxes(&layout, &mut out);
    Ok(out)
}

/// Flattens a laid-out tree into [`crate::render::NodeBox`] values.
fn collect_boxes(layout: &crate::render::layout::LayoutBox, out: &mut Vec<crate::render::NodeBox>) {
    out.push(crate::render::NodeBox {
        node: layout.node,
        x: layout.rect.x,
        y: layout.rect.y,
        width: layout.rect.width,
        height: layout.rect.height,
        visible: layout.style.visibility == crate::render::style::Visibility::Visible,
    });
    for item in &layout.items {
        if let crate::render::layout::PaintItem::Box(child) = item {
            collect_boxes(child, out);
        }
    }
}

/// Whether a viewport or scale value is usable.
fn valid_dimension(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

/// Paints a laid-out tree in CSS paint order: background, border, then
/// content (<https://drafts.csswg.org/css2/#painting-order>).
///
/// A hidden box skips its own background, border, and text, but its
/// descendants still paint: `visibility` is inherited and a descendant may
/// set `visible` again (`<https://drafts.csswg.org/css2/#propdef-visibility>`;
/// hidden text already carries an alpha-0 brush from the cascade).
fn paint(
    painter: &mut Painter,
    layout: &LayoutBox,
    fonts: &Fonts,
    images: &std::collections::HashMap<dom::NodeId, RasterImage>,
    dom: &Dom,
    styles: &std::collections::HashMap<dom::NodeId, crate::render::style::Style>,
) {
    if layout.style.opacity <= 0.0 {
        return;
    }
    let visible = layout.style.visibility == crate::render::style::Visibility::Visible;
    let style = &layout.style;
    let rect = layout.rect;
    let radius_basis = rect.width.min(rect.height);
    let radii = style
        .border_radius
        .map(|radius| radius.resolve(radius_basis));
    if visible && style.background.a > 0 {
        if radii.iter().any(|radius| *radius > 0.0) {
            painter.fill_rounded_rect(rect, radii, style.background);
        } else {
            painter.fill_rect(rect, style.background);
        }
    }

    if visible {
        paint_border(painter, rect, radii, style);
    }

    let clipped = style.overflow == crate::render::style::Overflow::Hidden;
    if clipped {
        painter.push_clip(layout.padding_box(), radii);
    }
    if visible {
        if let Some(image) = layout.node.and_then(|node| images.get(&node)) {
            painter.draw_image(layout.content_box(), image);
        } else if let Some(node) = layout
            .node
            .filter(|node| crate::render::svg::is_outer_svg(dom, *node))
        {
            crate::render::svg::paint(painter, dom, styles, node, layout.content_box());
        }
    }
    for item in &layout.items {
        match item {
            PaintItem::Box(child) => paint(painter, child, fonts, images, dom, styles),
            PaintItem::Glyphs(run) => paint_glyphs(painter, fonts, run),
        }
    }
    if clipped {
        painter.pop_clip();
    }
}

fn paint_border(
    painter: &mut Painter,
    rect: Rect,
    radii: [f32; 4],
    style: &crate::render::style::Style,
) {
    let border = style.border;
    let uniform_rounded = radii.iter().any(|radius| *radius > 0.0)
        && border.top.paints()
        && border.top == border.right
        && border.top == border.bottom
        && border.top == border.left;
    if uniform_rounded {
        painter.fill_rounded_border(rect, radii, border.top.width, border.top.color);
        return;
    }
    if border.top.paints() {
        painter.fill_rect(
            Rect::new(rect.x, rect.y, rect.width, border.top.width),
            border.top.color,
        );
    }
    if border.right.paints() {
        painter.fill_rect(
            Rect::new(
                rect.right() - border.right.width,
                rect.y + border.top.width,
                border.right.width,
                rect.height - border.top.width - border.bottom.width,
            ),
            border.right.color,
        );
    }
    if border.bottom.paints() {
        painter.fill_rect(
            Rect::new(
                rect.x,
                rect.bottom() - border.bottom.width,
                rect.width,
                border.bottom.width,
            ),
            border.bottom.color,
        );
    }
    if border.left.paints() {
        painter.fill_rect(
            Rect::new(
                rect.x,
                rect.y + border.top.width,
                border.left.width,
                rect.height - border.top.width - border.bottom.width,
            ),
            border.left.color,
        );
    }
}

fn paint_glyphs(painter: &mut Painter, fonts: &Fonts, run: &crate::render::text::PlacedRun) {
    painter.draw_glyphs(
        &run.glyphs,
        &fonts.outline_face(run.weight),
        run.size,
        run.color,
    );
    let thickness = (run.size * 0.06).max(1.0);
    match run.decoration {
        crate::render::style::TextDecoration::None => {}
        crate::render::style::TextDecoration::Underline => {
            painter.fill_rect(
                Rect::new(run.x, run.baseline + run.size * 0.1, run.width, thickness),
                run.color,
            );
        }
        crate::render::style::TextDecoration::LineThrough => {
            painter.fill_rect(
                Rect::new(run.x, run.baseline - run.size * 0.3, run.width, thickness),
                run.color,
            );
        }
    }
}
