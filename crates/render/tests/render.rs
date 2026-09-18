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


#[test]
fn paints_every_word_on_the_line() {
    // Regression test: the trailing-space trim used to discard a line-final
    // word, so only the first word painted.
    let dom = document(|dom, body| {
        let paragraph = dom.create_element(html_name("p"), Vec::new());
        dom.append(body, paragraph).expect("append p");
        let text = dom.create_text("Hello screenshot");
        dom.append(paragraph, text).expect("append text");
    });
    let image = render::render(&dom, &[], &render::RenderOptions {
        width: 400.0,
        height: 100.0,
        scale: 1.0,
    })
    .expect("render");
    let mut min_x = u32::MAX;
    let mut max_x = 0u32;
    let mut count = 0u32;
    for y in 0..image.height {
        for x in 0..image.width {
            let p = pixel(&image, x, y);
            if p[0] < 128 && p[1] < 128 && p[2] < 128 {
                count += 1;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
            }
        }
    }
    assert!(count > 200, "both words paint: {count} dark pixels");
    assert!(
        max_x - min_x > 100,
        "text spans both words: {min_x}..{max_x}"
    );
}

#[test]
fn floats_place_and_clear() {
    // Regression test: childless boxes routed through Taffy's measure path
    // once reported zero size, so floats vanished entirely.
    let dom = document(|dom, body| {
        let wrap = dom.create_element(
            html_name("div"),
            vec![attr("style", "width:360px;background:#eeeeee")],
        );
        dom.append(body, wrap).expect("append");
        let left = dom.create_element(
            html_name("div"),
            vec![attr("style", "float:left;width:100px;height:80px;background:#ff0000")],
        );
        let right = dom.create_element(
            html_name("div"),
            vec![attr("style", "float:right;width:100px;height:80px;background:#0000ff")],
        );
        let clear = dom.create_element(
            html_name("div"),
            vec![attr("style", "clear:both;height:20px;background:#00aa00")],
        );
        dom.append(wrap, left).expect("append");
        dom.append(wrap, right).expect("append");
        dom.append(wrap, clear).expect("append");
    });
    let sheet = "html, body { margin: 0; padding: 0; }".to_owned();
    let image = render::render(&dom, &[sheet], &render::RenderOptions {
        width: 400.0,
        height: 300.0,
        scale: 1.0,
    })
    .expect("render");
    let mut red = (u32::MAX, u32::MAX, 0u32, 0u32, 0u32);
    let mut blue = (u32::MAX, u32::MAX, 0u32, 0u32, 0u32);
    let mut green = (u32::MAX, u32::MAX, 0u32, 0u32, 0u32);
    for y in 0..image.height {
        for x in 0..image.width {
            let p = pixel(&image, x, y);
            let slot = if p[0] > 200 && p[1] < 80 && p[2] < 80 {
                Some(&mut red)
            } else if p[2] > 200 && p[0] < 80 && p[1] < 80 {
                Some(&mut blue)
            } else if p[1] > 100 && p[0] < 100 && p[2] < 100 {
                Some(&mut green)
            } else {
                None
            };
            if let Some(s) = slot {
                s.4 += 1;
                s.0 = s.0.min(x);
                s.1 = s.1.min(y);
                s.2 = s.2.max(x);
                s.3 = s.3.max(y);
            }
        }
    }
    println!("red {red:?} blue {blue:?} green {green:?}");
    assert_eq!((red.0, red.1, red.2, red.3), (0, 0, 99, 79));
    assert_eq!((blue.0, blue.1, blue.2, blue.3), (260, 0, 359, 79));
    assert_eq!((green.0, green.1, green.2, green.3), (0, 80, 359, 99));
}
