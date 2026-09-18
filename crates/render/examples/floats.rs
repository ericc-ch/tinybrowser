//! Renders a float + positioned layout page for visual checks.
//!
//! Throwaway visualizer, not a test: `cargo run -p render --example floats`.
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
    let body = dom.create_element(html_name("body"), Vec::new());
    dom.append(root, html).expect("append");
    dom.append(html, body).expect("append");

    let sheet = "html, body { margin: 0; padding: 0; } \
        #wrap { width: 360px; background: #eeeeee; } \
        #left { float: left; width: 100px; height: 80px; background: #ff0000; } \
        #right { float: right; width: 100px; height: 80px; background: #0000ff; } \
        #clear { clear: both; height: 20px; background: #00aa00; } \
        #abs { position: absolute; left: 200px; top: 10px; width: 60px; height: 60px; background: #ff8800; } \
        #flex { display: flex; gap: 8px; margin-top: 8px; } \
        .item { flex: 1; height: 30px; background: #8800ff; }"
        .to_owned();

    let wrap = dom.create_element(html_name("div"), vec![attr("id", "wrap")]);
    let left = dom.create_element(html_name("div"), vec![attr("id", "left")]);
    let right = dom.create_element(html_name("div"), vec![attr("id", "right")]);
    let clear = dom.create_element(html_name("div"), vec![attr("id", "clear")]);
    let absolute = dom.create_element(html_name("div"), vec![attr("id", "abs")]);
    let flex = dom.create_element(html_name("div"), vec![attr("id", "flex")]);
    dom.append(body, wrap).expect("append");
    dom.append(wrap, left).expect("append");
    dom.append(wrap, right).expect("append");
    dom.append(wrap, clear).expect("append");
    dom.append(body, absolute).expect("append");
    dom.append(body, flex).expect("append");
    for _ in 0..3 {
        let item = dom.create_element(html_name("div"), vec![attr("class", "item")]);
        dom.append(flex, item).expect("append");
    }

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
