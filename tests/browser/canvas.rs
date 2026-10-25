//! Canvas origin-cleanliness across an image response that lands on another
//! origin. The page and the image it redirects to are served on different
//! ports, so they are different origins.

use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use serde_json::json;

use crate::fixture::{Fixture, load_page, read_request_path};

/// One 1x1 red PNG.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

/// Draws each image onto a fresh canvas and reads the pixels back. A tainted
/// canvas throws `SecurityError` from `getImageData`; a clean one returns.
const PROBE: &str = r"(async () => {
  const settle = (id) => new Promise((resolve, reject) => {
    const image = document.getElementById(id);
    if (image.complete) {
      resolve();
      return;
    }
    image.addEventListener('load', resolve, { once: true });
    image.addEventListener('error', resolve, { once: true });
    setTimeout(() => reject(new Error('image did not settle')), 5000);
  });
  await Promise.all([settle('redirected'), settle('same')]);
  const probe = (id) => {
    const canvas = document.createElement('canvas');
    canvas.width = 1;
    canvas.height = 1;
    const context = canvas.getContext('2d');
    context.drawImage(document.getElementById(id), 0, 0);
    try {
      context.getImageData(0, 0, 1, 1);
      return 'clean';
    } catch (error) {
      return error.name;
    }
  };
  return JSON.stringify({ redirected: probe('redirected'), same: probe('same') });
})()";

fn bind() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("server bind");
    let address = listener.local_addr().expect("server addr");
    (listener, address)
}

/// Serves one response per request path for every connection. An unknown path
/// gets an empty 404 so closing the socket never sends RST.
fn serve(listener: TcpListener, routes: Vec<(&'static str, Vec<u8>)>) {
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let Some(path) = read_request_path(&mut stream) else {
                continue;
            };
            let response = routes.iter().find(|(route, _)| *route == path).map_or_else(
                || {
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                },
                |(_, response)| response.clone(),
            );
            let _ = stream.write_all(&response);
        }
    });
}

fn ok(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn redirect(location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

#[test]
fn cross_origin_redirect_taints_the_canvas() {
    let pixel = base64::engine::general_purpose::STANDARD
        .decode(PIXEL)
        .expect("png");

    let (cross_listener, cross) = bind();
    serve(
        cross_listener,
        vec![("/pixel.png", ok("image/png", &pixel))],
    );

    let (page_listener, page) = bind();
    let body = format!(
        "<!doctype html><img id=redirected src=\"http://{page}/redirect.png\">\
         <img id=same src=\"http://{page}/pixel.png\">"
    );
    serve(
        page_listener,
        vec![
            ("/", ok("text/html", body.as_bytes())),
            ("/pixel.png", ok("image/png", &pixel)),
            (
                "/redirect.png",
                redirect(&format!("http://{cross}/pixel.png")),
            ),
        ],
    );

    let (fixture, mut client, session) = open_page("tinybrowser-canvas-taint", page);
    let result = evaluate(&mut client, &session, PROBE);
    let result = result.as_str().expect("result");
    let result: serde_json::Value = serde_json::from_str(result).expect("json");
    assert_eq!(result["redirected"], json!("SecurityError"));
    assert_eq!(result["same"], json!("clean"));
    drop(fixture);
}

/// Starts the daemon, loads `page`'s root in a new target, and returns the
/// fixture that owns the daemon.
fn open_page(prefix: &str, page: SocketAddr) -> (Fixture, cdp::Client, String) {
    let mut fixture = Fixture::new(prefix);
    fixture.spawn_daemon();
    let _ = fixture.wait_json();
    let mut client = fixture.connect();
    let session = load_page(
        &mut client,
        &format!("http://{page}/"),
        Duration::from_secs(10),
    );
    (fixture, client, session)
}

fn evaluate(client: &mut cdp::Client, session: &str, expression: &str) -> serde_json::Value {
    client
        .call(
            "Runtime.evaluate",
            &json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
            Some(session),
        )
        .expect("evaluate")["result"]["value"]
        .clone()
}

#[test]
fn blob_encoders_convert_the_type_once() {
    let (page_listener, page) = bind();
    serve(
        page_listener,
        vec![("/", ok("text/html", b"<!doctype html><canvas id=canvas>"))],
    );
    let (fixture, mut client, session) = open_page("tinybrowser-canvas-blob", page);

    let result = evaluate(
        &mut client,
        &session,
        r"(() => {
          const converted = (run) => new Promise((resolve) => {
            let conversions = 0;
            const type = { toString() { conversions += 1; return 'image/png'; } };
            run(type).then(() => resolve(conversions));
          });
          const context = document.getElementById('canvas').getContext('2d');
          const offscreen = new OffscreenCanvas(1, 1);
          offscreen.getContext('2d');
          return Promise.all([
            converted((type) => new Promise((resolve) => context.canvas.toBlob(resolve, type))),
            converted((type) => offscreen.convertToBlob({ type })),
          ]);
        })()",
    );
    assert_eq!(result, json!([1, 1]));
    drop(fixture);
}
