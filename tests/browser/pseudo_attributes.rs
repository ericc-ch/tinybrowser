//! Style sheets named by `xml-stylesheet` processing instructions: the
//! attribute map, the data, and the loader must agree on what a sheet is.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::fixture::{evaluate, open_page};

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

/// A server for the test document and both sheets that records the paths it
/// was asked for and holds `/b.css` until [`Server::release_b`]. Each
/// connection gets its own thread, so a held request does not block the rest.
struct Server {
    address: SocketAddr,
    requested: Arc<Mutex<Vec<String>>>,
    released: Arc<(Mutex<bool>, Condvar)>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("server bind");
        let address = listener.local_addr().expect("server addr");
        let requested: Arc<Mutex<Vec<String>>> = Arc::default();
        let released: Arc<(Mutex<bool>, Condvar)> = Arc::default();
        let recorded = Arc::clone(&requested);
        let held = Arc::clone(&released);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    continue;
                };
                let recorded = Arc::clone(&recorded);
                let held = Arc::clone(&held);
                thread::spawn(move || serve(stream, &recorded, &held));
            }
        });
        Self {
            address,
            requested,
            released,
        }
    }

    fn requested(&self, path: &str) -> bool {
        self.requested
            .lock()
            .unwrap()
            .iter()
            .any(|requested| requested == path)
    }

    fn wait_requested(&self, path: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.requested(path) {
            assert!(Instant::now() < deadline, "no request for {path}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn release_b(&self) {
        let (lock, condvar) = &*self.released;
        *lock.lock().unwrap() = true;
        condvar.notify_all();
    }
}

fn serve(mut stream: TcpStream, requested: &Mutex<Vec<String>>, released: &(Mutex<bool>, Condvar)) {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => head.extend_from_slice(&chunk[..read]),
        }
    }
    let head = String::from_utf8_lossy(&head);
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
    requested.lock().unwrap().push(path.clone());
    if path == "/b.css" {
        let (lock, condvar) = released;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = condvar.wait(released).unwrap();
        }
    }
    let (content_type, body): (&str, &[u8]) = match path.as_str() {
        "/" => ("application/xml", DOCUMENT),
        "/a.css" => ("text/css", SHEET_A),
        "/b.css" => ("text/css", SHEET_B),
        _ => ("text/plain", b""),
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
}

#[test]
fn an_attribute_write_that_does_not_parse_requests_nothing() {
    let server = Server::start();
    let page = server.address;
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
    server.release_b();

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
    let server = Server::start();
    let page = server.address;
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
    server.wait_requested("/b.css");

    let expression = format!(
        r"{POLL}
(async () => {{
  document.firstChild.setAttribute('href', '/a.css');
  return await waitColor('{COLOR_A}');
}})()"
    );
    let restored = evaluate(&mut client, &session, &expression);
    assert_eq!(restored, json!(COLOR_A));

    server.release_b();
    thread::sleep(Duration::from_millis(500));
    let after_release = evaluate(
        &mut client,
        &session,
        "getComputedStyle(document.getElementById('p')).color",
    );
    assert_eq!(after_release, json!(COLOR_A));
}
