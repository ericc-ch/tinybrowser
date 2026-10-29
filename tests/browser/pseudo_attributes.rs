//! Style sheets named by `xml-stylesheet` processing instructions: the
//! attribute map, the data, and the loader must agree on what a sheet is.

use serde_json::json;

use crate::fixture::{evaluate, ok, open_page, serve_on, server};

/// The first sheet paints the paragraph one color, the second another.
const SHEET_A: &[u8] = b"p { color: rgb(1, 2, 3); }";
const SHEET_B: &[u8] = b"p { color: rgb(4, 5, 6); }";
const COLOR_A: &str = "rgb(1, 2, 3)";
const COLOR_B: &str = "rgb(4, 5, 6)";

/// An XML document whose prolog instruction loads `SHEET_A`.
const DOCUMENT: &[u8] = br#"<?xml-stylesheet href="/a.css"?><root xmlns="http://www.w3.org/1999/xhtml"><p id="p">x</p></root>"#;

/// Waits until the paragraph paints `expected`, or returns what it paints
/// when the budget runs out.
const POLL: &str = "
async function waitColor(expected) {
  for (let i = 0; i < 250; ++i) {
    if (getComputedStyle(document.getElementById('p')).color === expected) return expected;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  return getComputedStyle(document.getElementById('p')).color;
}
";

#[test]
fn an_attribute_write_that_does_not_parse_loads_nothing() {
    let (listener, page) = server();
    serve_on(
        listener,
        vec![
            ("/", ok("application/xml", DOCUMENT)),
            ("/a.css", ok("text/css", SHEET_A)),
            ("/b.css", ok("text/css", SHEET_B)),
        ],
    );
    let (_fixture, mut client, session) =
        open_page("tinybrowser-pi-attributes", &format!("http://{page}/"));

    // A `$` attribute serializes to data that is not a parse error-free
    // pseudo-attribute list, so the instruction names no sheet; fixing the
    // data afterwards must load one, which proves the loader ran.
    let expression = format!(
        r#"{POLL}
(async () => {{
  const before = await waitColor('{COLOR_A}');
  const pi = document.createProcessingInstruction('xml-stylesheet', '');
  pi.setAttribute('href', '/b.css');
  pi.setAttribute('$', '1');
  document.insertBefore(pi, document.documentElement);
  const unparsed = getComputedStyle(document.getElementById('p')).color;
  await new Promise((resolve) => setTimeout(resolve, 1000));
  const stillUnparsed = getComputedStyle(document.getElementById('p')).color;
  pi.data = 'href="/b.css"';
  const afterFix = await waitColor('{COLOR_B}');
  return [before, unparsed, stillUnparsed, afterFix];
}})()"#
    );
    let colors = evaluate(&mut client, &session, &expression);
    assert_eq!(colors, json!([COLOR_A, COLOR_A, COLOR_A, COLOR_B]));
}

#[test]
fn writing_a_connected_instruction_reloads_the_sheet() {
    let (listener, page) = server();
    serve_on(
        listener,
        vec![
            ("/", ok("application/xml", DOCUMENT)),
            ("/a.css", ok("text/css", SHEET_A)),
            ("/b.css", ok("text/css", SHEET_B)),
        ],
    );
    let (_fixture, mut client, session) =
        open_page("tinybrowser-pi-data", &format!("http://{page}/"));

    let expression = format!(
        r#"{POLL}
(async () => {{
  const before = await waitColor('{COLOR_A}');
  const pi = document.firstChild;
  pi.setAttribute('href', '/b.css');
  const afterAttribute = await waitColor('{COLOR_B}');
  pi.data = 'href="/a.css"';
  const afterData = await waitColor('{COLOR_A}');
  return [before, afterAttribute, afterData];
}})()"#
    );
    let colors = evaluate(&mut client, &session, &expression);
    assert_eq!(colors, json!([COLOR_A, COLOR_B, COLOR_A]));
}
