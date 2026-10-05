//! Blitz screenshots: HTML string through the Blitz stack into pixels.

#[cfg(test)]
use crate::render::providers::TinyNetProvider;
use crate::render::{RenderError, RgbaImage};

/// Largest viewport side in device pixels, matching the in-tree painter cap.
const MAX_SIDE: u32 = 4096;

/// Fallback base URL when the document URL cannot be a base.
pub(crate) const INVALID_BASE_URL: &str = "http://invalid/";

/// Base URL string for a Blitz document configuration.
pub(crate) fn blitz_base_url(url: &url::Url) -> String {
    if url.cannot_be_a_base() {
        INVALID_BASE_URL.to_owned()
    } else {
        url.as_str().to_owned()
    }
}

/// Renders `html` at `width` x `height` device pixels through Blitz.
///
/// External subresources are not fetched on this path (dummy providers), so
/// only inline content paints.
///
/// # Errors
///
/// [`RenderError::InvalidViewport`] when either side is zero.
/// [`RenderError::TooLarge`] when either side exceeds the pixel cap.
#[cfg(test)]
pub(crate) fn render_html(html: &str, width: u32, height: u32) -> Result<RgbaImage, RenderError> {
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
    paint(&mut base, width, height)
}

/// Runs one resolve-plus-delivery frame, returning true when settled.
///
/// Frames the Blitz event loop as caller-driven steps: resolve, deliver
/// arrived resources, then report whether anything is still pending (critical
/// resources or in-flight fetches). The caller sleeps between frames on its
/// own executor and paints with [`paint`] once this returns true (or its own
/// deadline hits, painting whatever settled).
#[cfg(test)]
pub(crate) fn resolve_frame(
    base: &mut blitz_dom::BaseDocument,
    net: &TinyNetProvider,
) -> bool {
    base.resolve(0.0);
    base.handle_messages();
    !base.has_pending_critical_resources() && net.in_flight() == 0
}

/// Paints an already-resolved document.
///
/// # Errors
///
/// [`RenderError::InvalidViewport`] when either side is zero.
/// [`RenderError::TooLarge`] when either side exceeds the pixel cap.
pub(crate) fn paint(
    base: &mut blitz_dom::BaseDocument,
    width: u32,
    height: u32,
) -> Result<RgbaImage, RenderError> {
    check_dims(width, height)?;
    let view_width = u16::try_from(width).map_err(|_| RenderError::TooLarge)?;
    let view_height = u16::try_from(height).map_err(|_| RenderError::TooLarge)?;
    let mut painter = anyrender_vello_cpu::VelloCpuScenePainter::new(view_width, view_height);
    blitz_paint::paint_scene(&mut painter, base, 1.0, width, height, 0, 0);
    let pixmap = painter.finish();
    let pixels = pixmap.data();
    let mut data = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        data.extend_from_slice(&over_white(*pixel));
    }
    Ok(RgbaImage {
        width,
        height,
        data,
    })
}

/// Rejects zero and over-cap viewports.
fn check_dims(width: u32, height: u32) -> Result<(), RenderError> {
    if width == 0 || height == 0 {
        return Err(RenderError::InvalidViewport);
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(RenderError::TooLarge);
    }
    Ok(())
}

/// Converts one premultiplied pixel to opaque straight-alpha `[r, g, b, 255]`,
/// compositing onto the white canvas (screenshots have no transparency).
fn over_white(pixel: color::PremulRgba8) -> [u8; 4] {
    // Premultiplied `c` over white: `c + 255 - a`, clamped. Opaque pixels
    // pass through; transparent pixels become white.
    let blend = |component: u8| component.saturating_add(u8::MAX - pixel.a);
    [blend(pixel.r), blend(pixel.g), blend(pixel.b), u8::MAX]
}

#[cfg(test)]
mod tests {
    use super::{paint, render_html, resolve_frame};
    use blitz_traits::net::NetHandler;
    use crate::render::providers::{TinyNetProvider, TinyShell};
    use std::io::{Read, Write};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

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

    #[test]
    fn external_stylesheet_applies_before_paint() {
        // The test plays the document's role: it serves the CSS itself and
        // delivers the queued fetch straight into the filed handler.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = "div{background:rgb(0,0,255)}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/css\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let net = Arc::new(TinyNetProvider::new(tx));
        let viewport = blitz_traits::shell::Viewport::new(
            200,
            120,
            1.0,
            blitz_traits::shell::ColorScheme::Light,
        );
        let config = blitz_dom::DocumentConfig {
            viewport: Some(viewport),
            net_provider: Some(net.clone()),
            shell_provider: Some(Arc::new(TinyShell)),
            ..blitz_dom::DocumentConfig::default()
        };
        let html = format!(
            "<!doctype html><html><head><link rel=stylesheet href=\"http://127.0.0.1:{port}/a.css\"></head><body style=\"margin:0\"><div style=\"width:200px;height:120px\"></div></body></html>"
        );
        let document = blitz_html::HtmlDocument::from_html(&html, config);
        let mut base: blitz_dom::BaseDocument = document.into();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            base.resolve(0.0);
            base.handle_messages();
            // Deliver every queued fetch the way the document would: read
            // the CSS over plain TCP and hand the bytes to the handler.
            while let Ok((fetch, handler)) = rx.try_recv() {
                let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))
                    .expect("css server");
                let request = format!(
                    "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
                    fetch.url.path()
                );
                stream
                    .write_all(request.as_bytes())
                    .expect("css request");
                let mut response = Vec::new();
                stream.read_to_end(&mut response).expect("css body");
                let body = response
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| response[index + 4..].to_vec())
                    .unwrap_or_default();
                Box::new(handler).bytes(
                    fetch.url.as_str().to_owned(),
                    blitz_traits::net::Bytes::from(body),
                );
            }
            base.handle_messages();
            if resolve_frame(&mut base, &net) || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let image = paint(&mut base, 200, 120).expect("blitz render");
        let middle = &image.data[((60 * 200 + 100) * 4) as usize..][..4];
        assert!(
            middle[2] > 200 && middle[3] == 255,
            "linked stylesheet paints blue, got {middle:?}"
        );
    }
}
