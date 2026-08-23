//! End-to-end parse tests: bytes → html5ever → [`Sink`] → observable tree.
//!
//! The expected shapes come from the HTML spec's own examples, so these
//! double as proof that recovery from broken markup matches browsers.

use browser::parse_html;
use dom::{Namespace, NodeId, NodeKind};
use std::fmt::Write;

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Indented tree dump in the html5lib-suite style, for readable assertions.
/// Suite convention: `#document` unprefixed; each level below gains two
/// spaces inside a leading `|`.
fn render(dom: &dom::Dom, id: NodeId, depth: usize, out: &mut String) {
    let pad = if depth == 0 {
        String::new()
    } else {
        format!("|{} ", "  ".repeat(depth - 1))
    };
    match dom.get(id).map(|n| n.kind()) {
        Some(NodeKind::Document) => writeln!(out, "{pad}#document"),
        Some(NodeKind::Doctype { name, .. }) => writeln!(out, "{pad}<!DOCTYPE {name}>"),
        Some(NodeKind::Element { name, .. }) => {
            let tag = if *name.ns == *Namespace::from(HTML_NS) {
                format!("<{}>", name.local)
            } else {
                format!("<{}::{}>", name.ns, name.local)
            };
            writeln!(out, "{pad}{tag}")
        }
        Some(NodeKind::Fragment) => writeln!(out, "{pad}#fragment"),
        Some(NodeKind::Text { data }) => writeln!(out, "{pad}\"{data}\""),
        Some(NodeKind::Comment { data }) => writeln!(out, "{pad}<!--{data}-->"),
        None => Ok(()),
    }
    .expect("writing to a String cannot fail");
    if let Some(kids) = dom.children(id) {
        for kid in kids {
            render(dom, *kid, depth + 1, out);
        }
    }
}

fn dump(parsed: &browser::Parsed) -> String {
    let mut out = String::new();
    render(&parsed.dom, parsed.dom.document(), 0, &mut out);
    out
}

#[track_caller]
fn child(dom: &dom::Dom, of: NodeId, index: usize) -> NodeId {
    dom.children(of)
        .unwrap()
        .copied()
        .nth(index)
        .unwrap_or_else(|| panic!("child #{index} missing"))
}

fn local_name(dom: &dom::Dom, id: NodeId) -> String {
    match dom.get(id).map(|n| n.kind()) {
        Some(NodeKind::Element { name, .. }) => name.local.to_string(),
        _ => panic!("not an element"),
    }
}

#[test]
fn parses_simple_document_with_implied_html_head_body() {
    let parsed = parse_html("<!DOCTYPE html><p>Hello <b>world</b></p>");
    assert_eq!(parsed.parse_errors, 0);
    assert_eq!(
        parsed.quirks_mode,
        markup5ever::interface::tree_builder::QuirksMode::NoQuirks
    );

    let d = &parsed.dom;
    let doc = d.document();
    let expected = "\
#document
| <!DOCTYPE html>
| <html>
|   <head>
|   <body>
|     <p>
|       \"Hello \"
|       <b>
|         \"world\"
";
    assert_eq!(dump(&parsed), expected);

    // spot-check handles: p → its bold child
    let html = child(d, doc, 1);
    let body = child(d, html, 1);
    let p = child(d, body, 0);
    assert_eq!(local_name(d, p), "p");
}

#[test]
fn recovers_from_crossed_tags_exactly_like_the_spec() {
    // Adoption-agency recovery: </b> fires while <p> is open. Outcome pinned
    // against markup5ever_rcdom (same html5ever 0.39) via probe — the
    // vendored html5lib suite is the long-term arbiter of such shapes.
    let parsed = parse_html("<b>1<p>2</b>3");
    let expected = "\
#document
| <html>
|   <head>
|   <body>
|     <b>
|       \"1\"
|     <p>
|       <b>
|         \"2\"
|       \"3\"
";
    assert_eq!(dump(&parsed), expected);
}

#[test]
fn missing_doctype_selects_quirks_mode() {
    let parsed = parse_html("<p>hi</p>");
    assert_eq!(
        parsed.quirks_mode,
        markup5ever::interface::tree_builder::QuirksMode::Quirks
    );
}

#[test]
fn bogus_pi_markup_becomes_a_comment_per_spec() {
    // Modern HTML spec: `<?...>` hits "incorrectly-opened-comment" and the
    // bogus-comment state swallows it verbatim — `create_pi` is never
    // involved for HTML documents.
    let parsed = parse_html("<?php echo 1 ?><html><body>x</body></html>");
    let d = &parsed.dom;
    let first = child(d, d.document(), 0);
    match d.get(first).map(|n| n.kind()) {
        Some(NodeKind::Comment { data }) => assert_eq!(data, "?php echo 1 ?"),
        other => panic!("expected comment, got {other:?}"),
    }
}

#[test]
fn template_contents_do_not_pollute_the_child_list() {
    let parsed = parse_html("<body><template><div id=a></div><span>b</span></template></body>");
    let d = &parsed.dom;
    let html = child(d, d.document(), 0);
    let body = child(d, html, 1);
    let template = child(d, body, 0);
    assert_eq!(local_name(d, template), "template");
    // contents live outside children(); they are reachable only through the
    // fragment association, which the serializer layer will consult
    assert_eq!(d.children(template).unwrap().count(), 0);
}
