//! Renders a grid layout page for visual checks.
//!
//! Throwaway visualizer, not a test: `cargo run -p render --example grid`.
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
        #grid { display: grid; grid-template-columns: 100px 1fr 1fr; \
                grid-template-rows: 60px 60px; gap: 8px; width: 376px; \
                background: #eeeeee; padding: 8px; } \
        #a { background: #ff0000; grid-row: span 2; } \
        #b { background: #00aa00; } \
        #c { background: #0000ff; grid-column: 3; grid-row: 1 / 3; } \
        #d { background: #ff8800; }"
        .to_owned();

    let grid = dom.create_element(html_name("div"), vec![attr("id", "grid")]);
    dom.append(body, grid).expect("append");
    for id in ["a", "b", "c", "d"] {
        let cell = dom.create_element(html_name("div"), vec![attr("id", id)]);
        let text = dom.create_text(id);
        dom.append(cell, text).expect("append");
        dom.append(grid, cell).expect("append");
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
