//! Pipeline smoke tests: a hand-built DOM renders to a correctly sized image
//! with painted content. Product-level conformance lives in the browser E2E
//! gate, not here.

use dom::{Attribute, Dom, LocalName, Namespace, QualName, html_namespace};

fn html_name(local: &str) -> QualName {
    QualName::new(None, html_namespace(), LocalName::from(local))
}

fn attr(local: &str, value: &str) -> Attribute {
    Attribute {
        name: QualName::new(None, Namespace::from(""), LocalName::from(local)),
        value: value.to_owned(),
    }
}

/// Builds the HTML skeleton with the given body children.
fn document(children: impl FnOnce(&mut Dom, dom::NodeId)) -> Dom {
    let mut dom = Dom::new();
    let root = dom.document();
    let html = dom.create_element(html_name("html"), Vec::new());
    let body = dom.create_element(html_name("body"), Vec::new());
    dom.append(root, html).expect("append html");
    dom.append(html, body).expect("append body");
    children(&mut dom, body);
    dom
}

fn pixel(image: &render::RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * image.width + x) * 4) as usize;
    [
        image.data[index],
        image.data[index + 1],
        image.data[index + 2],
        image.data[index + 3],
    ]
}

#[test]
fn renders_solid_background_and_text() {
    let dom = document(|dom, body| {
        let box_node = dom.create_element(
            html_name("div"),
            vec![attr(
                "style",
                "background: #ff0000; width: 100px; height: 40px",
            )],
        );
        dom.append(body, box_node).expect("append box");
        let paragraph = dom.create_element(html_name("p"), Vec::new());
        dom.append(body, paragraph).expect("append p");
        let text = dom.create_text("Hello tinybrowser");
        dom.append(paragraph, text).expect("append text");
    });

    let image = render::render(&dom, &[], &render::RenderOptions {
        width: 200.0,
        height: 120.0,
        scale: 1.0,
    })
    .expect("render");

    assert_eq!(image.width, 200);
    assert_eq!(image.height, 120);
    // Body margin is 8px, so the red box starts there.
    let red = pixel(&image, 20, 20);
    assert!(red[0] > 200 && red[1] < 60, "red box painted: {red:?}");
    // The paragraph text sits below the box and leaves non-white pixels.
    let mut dark = 0;
    for y in 50..110 {
        for x in 8..180 {
            let value = pixel(&image, x, y);
            if value[0] < 128 && value[1] < 128 && value[2] < 128 {
                dark += 1;
            }
        }
    }
    assert!(dark > 20, "text painted pixels: {dark}");
    // The far right edge stays white.
    assert_eq!(pixel(&image, 199, 119), [255, 255, 255, 255]);
}

#[test]
fn renders_link_style_from_stylesheet() {
    let dom = document(|dom, body| {
        let target = dom.create_element(html_name("div"), vec![attr("class", "hot")]);
        dom.append(body, target).expect("append");
    });
    let sheet = ".hot { background: #0000ff; width: 50px; height: 50px; }".to_owned();
    let image = render::render(&dom, &[sheet], &render::RenderOptions {
        width: 100.0,
        height: 100.0,
        scale: 1.0,
    })
    .expect("render");
    let blue = pixel(&image, 30, 30);
    assert!(blue[2] > 200 && blue[0] < 60, "class rule applied: {blue:?}");
}

#[test]
fn encodes_png() {
    let dom = document(|dom, body| {
        let node = dom.create_element(
            html_name("div"),
            vec![attr("style", "background: #00ff00; width: 20px; height: 20px")],
        );
        dom.append(body, node).expect("append");
    });
    let image = render::render(&dom, &[], &render::RenderOptions {
        width: 40.0,
        height: 40.0,
        scale: 1.0,
    })
    .expect("render");
    let png = render::encode_png(&image).expect("encode");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}
