//! The render pipeline: cascade styles over the DOM, build the box tree, lay
//! it out, and paint one image.

use dom::Dom;

use crate::render::font::Fonts;
use crate::render::geometry::Rect;
use crate::render::layout::{LayoutBox, PaintItem};
use crate::render::paint::Painter;
use crate::render::stylo::style_document;
use crate::render::tree;
use crate::render::{RenderError, RenderOptions, RgbaImage};

/// Renders `dom` into one image.
///
/// # Errors
///
/// See [`RenderError`].
pub(crate) fn render(
    dom: &Dom,
    stylesheets: &[String],
    options: &RenderOptions,
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
    let styles = style_document(dom, stylesheets, viewport_width, viewport_height);

    // 3. Build and lay out the box tree through Taffy.
    let root = tree::build(dom, &styles);
    let layout = crate::render::boxes::layout_root(&root, &fonts, viewport_width, viewport_height);

    // 4. Paint once, then encode from the caller.
    let width = crate::render::device_pixels((viewport_width * options.scale).round().max(1.0));
    let height = crate::render::device_pixels((viewport_height * options.scale).round().max(1.0));
    let mut painter = Painter::new(width, height)?;
    paint(&mut painter, &layout, &fonts);
    Ok(painter.into_image())
}

/// Lays `dom` out without painting and returns every box in tree order, for
/// script geometry (`getBoundingClientRect`, hit testing).
pub(crate) fn boxes(
    dom: &Dom,
    stylesheets: &[String],
    options: &RenderOptions,
) -> Result<Vec<crate::render::NodeBox>, RenderError> {
    if !valid_dimension(options.width)
        || !valid_dimension(options.height)
        || !valid_dimension(options.scale)
    {
        return Err(RenderError::InvalidViewport);
    }
    let fonts = Fonts::load()?;
    let styles = style_document(dom, stylesheets, options.width, options.height);
    let root = tree::build(dom, &styles);
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
fn paint(painter: &mut Painter, layout: &LayoutBox, fonts: &Fonts) {
    let visible = layout.style.visibility == crate::render::style::Visibility::Visible;
    let style = &layout.style;
    let rect = layout.rect;
    if visible && style.background.a > 0 {
        painter.fill_rect(rect, style.background);
    }

    let border = style.border;
    if visible {
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

    let clipped = style.overflow == crate::render::style::Overflow::Hidden;
    if clipped {
        painter.push_clip(layout.padding_box());
    }
    for item in &layout.items {
        match item {
            PaintItem::Box(child) => paint(painter, child, fonts),
            PaintItem::Glyphs(run) => {
                painter.draw_glyphs(
                    &run.glyphs,
                    &fonts.outline_face(run.weight),
                    run.size,
                    run.color,
                );
                match run.decoration {
                    crate::render::style::TextDecoration::None => {}
                    crate::render::style::TextDecoration::Underline => {
                        painter.fill_rect(
                            Rect::new(
                                run.x,
                                run.baseline + run.size * 0.1,
                                run.width,
                                (run.size * 0.06).max(1.0),
                            ),
                            run.color,
                        );
                    }
                    crate::render::style::TextDecoration::LineThrough => {
                        painter.fill_rect(
                            Rect::new(
                                run.x,
                                run.baseline - run.size * 0.3,
                                run.width,
                                (run.size * 0.06).max(1.0),
                            ),
                            run.color,
                        );
                    }
                }
            }
        }
    }
    if clipped {
        painter.pop_clip();
    }
}
