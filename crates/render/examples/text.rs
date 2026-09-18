//! Renders text features (bold, wrap, underline, breaks) for visual checks.
//!
//! Throwaway visualizer, not a test: `cargo run -p render --example text`.
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

    let sheet = "html, body { margin: 0; padding: 8px; } \
        #wrap { width: 200px; background: #f0f0f0; } \
        #u { text-decoration: underline; } \
        #c { text-align: center; }"
        .to_owned();

    let wrap = dom.create_element(html_name("div"), vec![attr("id", "wrap")]);
    dom.append(body, wrap).expect("append");

    let p1 = dom.create_element(html_name("p"), Vec::new());
    let t1 = dom.create_text("plain and bold words here");
    dom.append(p1, t1).expect("append");
    let b = dom.create_element(html_name("b"), Vec::new());
    let t2 = dom.create_text("bold tail");
    dom.append(b, t2).expect("append");
    dom.append(p1, b).expect("append");
    dom.append(wrap, p1).expect("append");

    let p2 = dom.create_element(html_name("p"), vec![attr("id", "u")]);
    let t3 = dom.create_text("underlined link text");
    dom.append(p2, t3).expect("append");
    dom.append(wrap, p2).expect("append");

    let p3 = dom.create_element(html_name("p"), vec![attr("id", "c")]);
    let t4 = dom.create_text("centered");
    dom.append(p3, t4).expect("append");
    dom.append(wrap, p3).expect("append");

    let p4 = dom.create_element(html_name("p"), Vec::new());
    let t5 = dom.create_text("first");
    let br = dom.create_element(html_name("br"), Vec::new());
    let t6 = dom.create_text("second");
    dom.append(p4, t5).expect("append");
    dom.append(p4, br).expect("append");
    dom.append(p4, t6).expect("append");
    dom.append(wrap, p4).expect("append");

    let image = render::render(
        &dom,
        &[sheet],
        &render::RenderOptions {
            width: 400.0,
            height: 400.0,
            scale: 1.0,
        },
    )
    .expect("render");
    let png = render::encode_png(&image).expect("encode");
    std::fs::write(&out, png).expect("write");
    println!("wrote {out}");
}
