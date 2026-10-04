//! Blitz proof-of-render: HTML string through the Blitz stack into pixels.
//!
//! Scaffold for the full cutover: parses with `blitz-html`, resolves style
//! and layout on the `BaseDocument`, paints the scene through
//! `anyrender_vello_cpu`, and converts the premultiplied output to our
//! straight-alpha [`RgbaImage`](crate::render::RgbaImage). Replaces
//! [`cascade`](crate::render::cascade) once providers and bindings move over.

use crate::render::{RenderError, RgbaImage};

/// Largest viewport side in device pixels, matching the in-tree painter cap.
const MAX_SIDE: u32 = 4096;

/// Renders `html` at `width` x `height` device pixels through Blitz.
///
/// # Errors
///
/// [`RenderError::InvalidViewport`] when either side is zero.
/// [`RenderError::TooLarge`] when either side exceeds the pixel cap.
pub(crate) fn render_html(html: &str, width: u32, height: u32) -> Result<RgbaImage, RenderError> {
    if width == 0 || height == 0 {
        return Err(RenderError::InvalidViewport);
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(RenderError::TooLarge);
    }
    let view_width = u16::try_from(width).map_err(|_| RenderError::TooLarge)?;
    let view_height = u16::try_from(height).map_err(|_| RenderError::TooLarge)?;
    let viewport = blitz_traits::shell::Viewport::new(
        width,
        height,
        1.0,
        blitz_traits::shell::ColorScheme::Light,
    );
    let config = blitz_dom::DocumentConfig {
        viewport: Some(viewport),
        ..blitz_dom::DocumentConfig::default()
    };
    let document = blitz_html::HtmlDocument::from_html(html, config);
    let mut base: blitz_dom::BaseDocument = document.into();
    base.resolve(0.0);
    let mut painter =
        anyrender_vello_cpu::VelloCpuScenePainter::new(view_width, view_height);
    blitz_paint::paint_scene(&mut painter, &mut base, 1.0, width, height, 0, 0);
    let pixmap = painter.finish();
    let pixels = pixmap.data();
    let mut data = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        data.extend_from_slice(&unpremultiply(*pixel));
    }
    Ok(RgbaImage {
        width,
        height,
        data,
    })
}

/// Converts one premultiplied pixel to straight-alpha `[r, g, b, a]`.
fn unpremultiply(pixel: color::PremulRgba8) -> [u8; 4] {
    if pixel.a == 0 {
        return [0, 0, 0, 0];
    }
    if pixel.a == u8::MAX {
        return [pixel.r, pixel.g, pixel.b, pixel.a];
    }
    let alpha = u32::from(pixel.a);
    let unpremultiply = |component: u8| {
        let straight = (u32::from(component) * 255 + alpha / 2) / alpha;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "rounded unpremultiply of clamped u8 inputs never exceeds 255"
        )]
        let narrowed = straight as u8;
        narrowed
    };
    [
        unpremultiply(pixel.r),
        unpremultiply(pixel.g),
        unpremultiply(pixel.b),
        pixel.a,
    ]
}

#[cfg(test)]
mod tests {
    use super::render_html;

    #[test]
    fn linear_gradient_paints_endpoints() {
        let image = render_html(
            "<!doctype html><html><body style=\"margin:0\"><div style=\"width:200px;height:120px;background:linear-gradient(red,blue)\"></div></body></html>",
            200,
            120,
        )
        .expect("blitz render");
        assert_eq!((image.width, image.height), (200, 120));
        let top = &image.data[0..4];
        let bottom_start = (119 * 200 * 4) as usize;
        let bottom = &image.data[bottom_start..bottom_start + 4];
        assert!(
            top[0] > 200 && top[3] == 255,
            "gradient starts red, got {top:?}"
        );
        assert!(
            bottom[2] > 200 && bottom[3] == 255,
            "gradient ends blue, got {bottom:?}"
        );
    }
}
