//! Pipeline smoke tests: a hand-built DOM renders to an image of the
//! requested size and encodes as PNG. Web-platform behavior is WPT's job
//! (`tools/wpt/run`, reftests for pixels); the browser E2E gate
//! (`tools/playwright/run`) exercises the product path end to end.

use dom::Dom;

#[test]
fn renders_requested_viewport() {
    let dom = Dom::new();
    let image = render::render(
        &dom,
        &[],
        &render::RenderOptions {
            width: 200.0,
            height: 120.0,
            scale: 1.0,
        },
    )
    .expect("render");
    assert_eq!((image.width, image.height), (200, 120));
    assert_eq!(image.data.len(), 200 * 120 * 4);
    assert!(
        image.data.iter().all(|byte| *byte == 255),
        "an empty document paints an opaque white viewport"
    );
}

#[test]
fn encodes_png() {
    let dom = Dom::new();
    let image = render::render(
        &dom,
        &[],
        &render::RenderOptions {
            width: 16.0,
            height: 8.0,
            scale: 1.0,
        },
    )
    .expect("render");
    let png = render::encode_png(&image).expect("encode");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}
