//! Style sheets named by `xml-stylesheet` processing instructions: the
//! attribute map, the data, and the loader must agree on what a sheet is.

use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::fixture::{TestServer, evaluate, open_page};

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

fn server() -> TestServer {
    TestServer::start(
        vec![
            ("/", "application/xml", DOCUMENT),
            ("/a.css", "text/css", SHEET_A),
            ("/b.css", "text/css", SHEET_B),
        ],
        Some("/b.css"),
    )
}

#[test]
fn an_attribute_write_that_does_not_parse_requests_nothing() {
    let server = server();
    let page = server.address();
    let (_fixture, mut client, session) =
        open_page("tinybrowser-pi-attributes", &format!("http://{page}/"));

    // A `$` attribute serializes to data that is not a parse-error-free
    // pseudo-attribute list. The instruction keeps the map the write gave it
    // and requests no sheet. Fixing the data afterwards loads one, which
    // proves the loader ran.
    let expression = format!(
        r"{POLL}
(async () => {{
  const before = await waitColor('{COLOR_A}');
  const pi = document.createProcessingInstruction('xml-stylesheet', '');
  pi.setAttribute('href', '/b.css');
  pi.setAttribute('$', '1');
  document.insertBefore(pi, document.documentElement);
  window.__pi = pi;
  await new Promise((resolve) => setTimeout(resolve, 500));
  return [
    before,
    getComputedStyle(document.getElementById('p')).color,
    pi.getAttribute('href'),
    pi.getAttribute('$'),
    pi.data,
  ];
}})()"
    );
    let state = evaluate(&mut client, &session, &expression);
    assert_eq!(
        state,
        json!([COLOR_A, COLOR_A, "/b.css", "1", r#"href="/b.css" $="1""#])
    );
    assert!(
        !server.requested("/b.css"),
        "data that does not parse must request no sheet"
    );
    server.release();

    let expression = format!(
        r#"{POLL}
(async () => {{
  window.__pi.data = 'href="/b.css"';
  return await waitColor('{COLOR_B}');
}})()"#
    );
    let color = evaluate(&mut client, &session, &expression);
    assert_eq!(color, json!(COLOR_B));
    assert!(server.requested("/b.css"));
}

#[test]
fn a_stale_response_never_installs_a_sheet() {
    let server = server();
    let page = server.address();
    let (_fixture, mut client, session) =
        open_page("tinybrowser-pi-stale", &format!("http://{page}/"));

    // Point the connected instruction at the held sheet, then at the first
    // one again while its response is still in flight.
    let expression = format!(
        r"{POLL}
(async () => {{
  const before = await waitColor('{COLOR_A}');
  document.firstChild.setAttribute('href', '/b.css');
  return before;
}})()"
    );
    let before = evaluate(&mut client, &session, &expression);
    assert_eq!(before, json!(COLOR_A));
    server.wait("/b.css");

    let expression = format!(
        r"{POLL}
(async () => {{
  document.firstChild.setAttribute('href', '/a.css');
  return await waitColor('{COLOR_A}');
}})()"
    );
    let restored = evaluate(&mut client, &session, &expression);
    assert_eq!(restored, json!(COLOR_A));

    server.release();
    thread::sleep(Duration::from_millis(500));
    let after_release = evaluate(
        &mut client,
        &session,
        "getComputedStyle(document.getElementById('p')).color",
    );
    assert_eq!(after_release, json!(COLOR_A));
}
