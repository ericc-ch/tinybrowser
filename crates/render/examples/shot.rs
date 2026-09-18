//! Renders the Playwright screenshot fixture to a PNG for visual checks.
//!
//! Throwaway visualizer, not a test: `cargo run -p render --example shot`.
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

fn main() {
    let out = std::env::args().nth(1).expect("output path");
    let mut dom = Dom::new();
    let root = dom.document();
    let html = dom.create_element(html_name("html"), Vec::new());
    let head = dom.create_element(html_name("head"), Vec::new());
    let style = dom.create_element(html_name("style"), Vec::new());
    let css = dom.create_text(
        "html, body { margin: 0; padding: 0; } \
         #red { background: #ff0000; width: 100px; height: 50px; } \
         #blue { background: #0000ff; width: 50px; height: 50px; } \
         p { margin: 16px 0 0 0; font-size: 20px; }",
    );
    let body = dom.create_element(html_name("body"), Vec::new());
    let red = dom.create_element(html_name("div"), vec![attr("id", "red")]);
    let blue = dom.create_element(html_name("div"), vec![attr("id", "blue")]);
    let paragraph = dom.create_element(html_name("p"), Vec::new());
    let text = dom.create_text("Hello screenshot");
    dom.append(root, html).expect("append");
    dom.append(html, head).expect("append");
    dom.append(head, style).expect("append");
    dom.append(style, css).expect("append");
    dom.append(html, body).expect("append");
    dom.append(body, red).expect("append");
    dom.append(body, blue).expect("append");
    dom.append(body, paragraph).expect("append");
    dom.append(paragraph, text).expect("append");

    // The render crate takes stylesheets as strings; the engine collects
    // `<style>` and `<link>` sheets, so pass the same text here.
    let sheet = "html, body { margin: 0; padding: 0; } \
         #red { background: #ff0000; width: 100px; height: 50px; } \
         #blue { background: #0000ff; width: 50px; height: 50px; } \
         p { margin: 16px 0 0 0; font-size: 20px; }"
        .to_owned();
    let image = render::render(
        &dom,
        &[sheet],
        &render::RenderOptions {
            width: 400.0,
            height: 300.0,
            scale: 1.0,
        },
    )
    .expect("render");
    let png = render::encode_png(&image).expect("encode");
    std::fs::write(&out, png).expect("write");
    println!("wrote {out}");
}
