use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use common::Fixture;
use serde_json::{Value, json};

mod common;

fn start(extra_args: Vec<String>) -> (String, Fixture) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let mut fixture = Fixture::new("tinybrowser-webdriver");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tinybrowser"));
    command.args(["webdriver", &format!("--port={port}")]);
    command.args(extra_args);
    let child = command
        .env("XDG_RUNTIME_DIR", &fixture.runtime)
        .env("XDG_DATA_HOME", &fixture.data)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn WebDriver");
    fixture.children.push(child);
    (format!("127.0.0.1:{port}"), fixture)
}

fn request(addr: &str, method: &str, path: &str, body: Option<&str>) -> Value {
    let payload = body.unwrap_or("");
    let mut last_error = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match TcpStream::connect(addr) {
            Ok(mut stream) => {
                let req = format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                stream.write_all(req.as_bytes()).expect("write");
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).expect("read");
                let text = String::from_utf8_lossy(&buf);
                let json_body = text.split("\r\n\r\n").nth(1).expect("http body");
                return serde_json::from_str(json_body).expect("json");
            }
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    panic!("webdriver not reachable: {last_error:?}");
}

#[test]
fn session_execute_script_roundtrip() {
    let (addr, _fixture) = start(Vec::new());

    let status = request(&addr, "GET", "/status", None);
    assert_eq!(status["value"]["ready"], json!(true));

    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();

    let added = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return 1+1","args":[]}"#),
    );
    assert_eq!(added["value"].as_f64(), Some(2.0));

    let nested = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return [1,['x']]","args":[]}"#),
    );
    assert_eq!(nested["value"], json!([1.0, ["x"]]));

    let async_result = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/async"),
        Some(r#"{"script":"arguments[0](3)","args":[]}"#),
    );
    assert_eq!(async_result["value"].as_f64(), Some(3.0));

    let delayed = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/async"),
        Some(
            r#"{"script":"var cb = arguments[0]; setTimeout(function() { cb(4); }, 0);","args":[]}"#,
        ),
    );
    assert_eq!(delayed["value"].as_f64(), Some(4.0));

    let nan = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return NaN","args":[]}"#),
    );
    assert_eq!(nan["value"], json!(null));

    let enumerated = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(
            r#"{"script":"return Object.keys(globalThis).filter(function(k){return k.indexOf('__tb_webdriver')===0||k.indexOf('__wd_')===0})","args":[]}"#,
        ),
    );
    assert_eq!(enumerated["value"], json!([]));
    let click_type = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return typeof __tb_webdriver_click","args":[]}"#),
    );
    assert_eq!(click_type["value"], json!("function"));
}

#[test]
fn execute_sync_waits_for_returned_promise_or_script_timeout() {
    let (addr, _fixture) = start(Vec::new());

    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();

    let resolved = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return Promise.resolve(7)","args":[]}"#),
    );
    assert_eq!(resolved["value"].as_f64(), Some(7.0));

    let delayed = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(
            r#"{"script":"return new Promise(function(resolve) { setTimeout(function() { resolve(8); }, 0); })","args":[]}"#,
        ),
    );
    assert_eq!(delayed["value"].as_f64(), Some(8.0));

    let rejected = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return Promise.reject()","args":[]}"#),
    );
    assert_eq!(rejected["value"]["error"], json!("javascript error"));

    let empty_reject = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return Promise.reject('')","args":[]}"#),
    );
    assert_eq!(empty_reject["value"]["error"], json!("javascript error"));

    let wait_object = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return { __wd_wait: true }","args":[]}"#),
    );
    assert_eq!(wait_object["value"], json!({"__wd_wait": true}));

    request(
        &addr,
        "POST",
        &format!("/session/{id}/timeouts"),
        Some(r#"{"script":200}"#),
    );
    let started = Instant::now();
    let timed_out = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(
            r#"{"script":"return new Promise(function(resolve) { setTimeout(function() { resolve(1); }, 30000); })","args":[]}"#,
        ),
    );
    assert_eq!(timed_out["value"]["error"], json!("script timeout"));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "script timeout waited {:?}",
        started.elapsed()
    );

    request(
        &addr,
        "POST",
        &format!("/session/{id}/timeouts"),
        Some(r#"{"script":400}"#),
    );
    let started = Instant::now();
    let remaining_budget = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(
            r#"{"script":"var t = Date.now(); while (Date.now() - t < 250) {} return new Promise(function(resolve) { setTimeout(function() { resolve(1); }, 250); });","args":[]}"#,
        ),
    );
    assert_eq!(remaining_budget["value"]["error"], json!("script timeout"));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shared script timeout waited {:?}",
        started.elapsed()
    );
}

#[test]
fn navigate_returns_after_load_not_after_timers() {
    let server_listener = TcpListener::bind("127.0.0.1:0").expect("server bind");
    let server_addr = server_listener.local_addr().expect("server addr");
    let server = thread::spawn(move || {
        let (mut navigation, _) = server_listener.accept().expect("navigation");
        let mut head = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = navigation.read(&mut chunk).expect("request");
            assert_ne!(read, 0);
            head.extend_from_slice(&chunk[..read]);
        }
        let body = b"<!doctype html><script>window.early = 1; setTimeout(function() { window.late = 1; }, 30000);</script>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        navigation.write_all(response.as_bytes()).expect("head");
        navigation.write_all(body).expect("body");
    });

    let (addr, _fixture) = start(Vec::new());

    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();

    let started = Instant::now();
    let navigated = request(
        &addr,
        "POST",
        &format!("/session/{id}/url"),
        Some(&format!(r#"{{"url":"http://{server_addr}/"}}"#)),
    );
    assert_eq!(navigated["value"], json!(null));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "navigate waited for host timers"
    );

    let early = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return window.early","args":[]}"#),
    );
    assert_eq!(early["value"].as_f64(), Some(1.0));

    let late = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return typeof window.late","args":[]}"#),
    );
    assert_eq!(late["value"], json!("undefined"));
    server.join().expect("server");
}

#[test]
fn new_window_uses_builder_resolve_map() {
    let server_listener = TcpListener::bind("127.0.0.1:0").expect("server bind");
    let server_addr = server_listener.local_addr().expect("server addr");
    let server = thread::spawn(move || {
        let (mut navigation, _) = server_listener.accept().expect("navigation");
        let mut head = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = navigation.read(&mut chunk).expect("request");
            assert_ne!(read, 0);
            head.extend_from_slice(&chunk[..read]);
        }
        let body = b"<!doctype html><title>mapped</title>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        navigation.write_all(response.as_bytes()).expect("head");
        navigation.write_all(body).expect("body");
    });

    let (addr, _fixture) = start(vec!["--resolve=*.test=127.0.0.1".to_owned()]);

    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();

    let opened = request(
        &addr,
        "POST",
        &format!("/session/{id}/window/new"),
        Some("{}"),
    );
    let handle = opened["value"]["handle"]
        .as_str()
        .expect("handle")
        .to_owned();
    request(
        &addr,
        "POST",
        &format!("/session/{id}/window"),
        Some(&format!(r#"{{"handle":"{handle}"}}"#)),
    );

    let navigated = request(
        &addr,
        "POST",
        &format!("/session/{id}/url"),
        Some(&format!(
            r#"{{"url":"http://web-platform.test:{}/"}}"#,
            server_addr.port()
        )),
    );
    assert_eq!(navigated["value"], json!(null));

    let current = request(&addr, "GET", &format!("/session/{id}/url"), None);
    assert_eq!(
        current["value"].as_str().expect("url"),
        format!("http://web-platform.test:{}/", server_addr.port())
    );
    server.join().expect("server");
}

#[test]
fn one_session_delete_closes_tabs_close_last_window_invalidates() {
    let (addr, _fixture) = start(Vec::new());

    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let second = request(&addr, "POST", "/session", Some("{}"));
    assert_eq!(second["value"]["error"], json!("session not created"));

    request(
        &addr,
        "POST",
        &format!("/session/{id}/window/new"),
        Some("{}"),
    );
    request(&addr, "DELETE", &format!("/session/{id}"), None);
    let gone = request(&addr, "GET", &format!("/session/{id}/window"), None);
    assert_eq!(gone["value"]["error"], json!("invalid session id"));

    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let handles = request(&addr, "GET", &format!("/session/{id}/window/handles"), None);
    assert_eq!(
        handles["value"].as_array().map(Vec::len),
        Some(1),
        "a new session starts with one window after DELETE closed the old tabs"
    );
    let closed = request(&addr, "DELETE", &format!("/session/{id}/window"), None);
    assert_eq!(closed["value"], json!([]));
    let invalid = request(&addr, "GET", &format!("/session/{id}/window"), None);
    assert_eq!(invalid["value"]["error"], json!("invalid session id"));
}

const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";

/// Serves one page over HTTP for any request until the test process exits.
fn spawn_page(html: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind page server");
    let port = listener.local_addr().expect("page addr").port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 1024];
            let _received = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                html.len(),
                html
            );
            let _written = stream.write_all(response.as_bytes());
        }
    });
    port
}

fn create_session(addr: &str) -> String {
    let created = request(addr, "POST", "/session", Some("{}"));
    created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned()
}

fn navigate(addr: &str, id: &str, url: &str) {
    let navigated = request(
        addr,
        "POST",
        &format!("/session/{id}/url"),
        Some(&json!({ "url": url }).to_string()),
    );
    assert_eq!(navigated["value"], Value::Null);
}

fn find(addr: &str, id: &str, selector: &str) -> String {
    let body = json!({"using": "css selector", "value": selector}).to_string();
    let found = request(addr, "POST", &format!("/session/{id}/element"), Some(&body));
    found["value"][ELEMENT_KEY]
        .as_str()
        .expect("element id")
        .to_owned()
}

#[test]
fn element_roundtrip() {
    let page_port = spawn_page("<!doctype html><button id=b>B</button><input id=i>");
    let (addr, _fixture) = start(Vec::new());
    let id = create_session(&addr);
    navigate(&addr, &id, &format!("http://127.0.0.1:{page_port}/"));

    // Hold the page objects, so their wrappers survive between calls, and
    // record whether the driver click is trusted.
    let held = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(
            r#"{"script":"window.__clicks=0; document.getElementById('b').addEventListener('click', e => { window.__clicks += e.isTrusted ? 10 : 1 }); window.__input = document.getElementById('i'); return 1","args":[]}"#,
        ),
    );
    assert_eq!(held["value"].as_f64(), Some(1.0));

    let button = find(&addr, &id, "#b");
    let clicked = request(
        &addr,
        "POST",
        &format!("/session/{id}/element/{button}/click"),
        Some("{}"),
    );
    assert_eq!(clicked["value"], Value::Null);
    let clicks = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return window.__clicks","args":[]}"#),
    );
    assert_eq!(clicks["value"].as_f64(), Some(10.0));

    let input = find(&addr, &id, "#i");
    let sent = request(
        &addr,
        "POST",
        &format!("/session/{id}/element/{input}/value"),
        Some(r#"{"text":"hi"}"#),
    );
    assert_eq!(sent["value"], Value::Null);
    let value = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return window.__input.value","args":[]}"#),
    );
    assert_eq!(value["value"], json!("hi"));

    // A reference returned by execute_script is a valid element reference.
    let script_ref = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return document.getElementById('b')","args":[]}"#),
    );
    let script_ref = script_ref["value"][ELEMENT_KEY]
        .as_str()
        .expect("script element id")
        .to_owned();
    let clicked_again = request(
        &addr,
        "POST",
        &format!("/session/{id}/element/{script_ref}/click"),
        Some("{}"),
    );
    assert_eq!(clicked_again["value"], Value::Null);

    // Find Elements returns every match, not just the first.
    let many = request(
        &addr,
        "POST",
        &format!("/session/{id}/elements"),
        Some(r#"{"using":"css selector","value":"*"}"#),
    );
    assert!(
        many["value"]
            .as_array()
            .is_some_and(|items| items.len() > 1)
    );

    // An invalid selector is `invalid selector`, not a javascript error.
    let bad = request(
        &addr,
        "POST",
        &format!("/session/{id}/element"),
        Some(r#"{"using":"css selector","value":"??"}"#),
    );
    assert_eq!(bad["value"]["error"], json!("invalid selector"));

    // The virtual rectangle is present for any live element.
    let rect = request(
        &addr,
        "GET",
        &format!("/session/{id}/element/{button}/rect"),
        None,
    );
    assert!(
        rect["value"]["width"]
            .as_f64()
            .is_some_and(|width| width > 0.0)
    );
}

#[test]
fn cookie_roundtrip() {
    let page_port = spawn_page("<!doctype html><title>cookies</title>");
    let (addr, _fixture) = start(Vec::new());
    let id = create_session(&addr);
    navigate(&addr, &id, &format!("http://127.0.0.1:{page_port}/"));

    let script_set = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"document.cookie='a=1; path=/'; return document.cookie","args":[]}"#),
    );
    assert_eq!(script_set["value"], json!("a=1"));
    let named = request(&addr, "GET", &format!("/session/{id}/cookie/a"), None);
    assert_eq!(named["value"]["value"], json!("1"));
    // No SameSite attribute serializes as "None".
    assert_eq!(named["value"]["sameSite"], json!("None"));
    let added = request(
        &addr,
        "POST",
        &format!("/session/{id}/cookie"),
        Some(r#"{"cookie":{"name":"h","value":"2","path":"/","httpOnly":true}}"#),
    );
    assert_eq!(added["value"], Value::Null);
    let http_only = request(&addr, "GET", &format!("/session/{id}/cookie/h"), None);
    assert_eq!(http_only["value"]["httpOnly"], json!(true));
    let all = request(&addr, "GET", &format!("/session/{id}/cookie"), None);
    assert_eq!(all["value"].as_array().map(Vec::len), Some(2));
    let cleared = request(&addr, "DELETE", &format!("/session/{id}/cookie"), None);
    assert_eq!(cleared["value"], Value::Null);
    let empty = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"return document.cookie","args":[]}"#),
    );
    assert_eq!(empty["value"], json!(""));
}

#[test]
fn window_rect_roundtrip() {
    let (addr, _fixture) = start(Vec::new());
    let id = create_session(&addr);

    let initial = request(&addr, "GET", &format!("/session/{id}/window/rect"), None);
    assert_eq!(initial["value"]["width"], json!(800));
    let set = request(
        &addr,
        "POST",
        &format!("/session/{id}/window/rect"),
        Some(r#"{"x":150,"y":175}"#),
    );
    assert_eq!(set["value"]["x"], json!(150));
    assert_eq!(set["value"]["y"], json!(175));
    let got = request(&addr, "GET", &format!("/session/{id}/window/rect"), None);
    assert_eq!(got["value"]["x"], json!(150));
    assert_eq!(got["value"]["y"], json!(175));

    // An out-of-range value is rejected and does not partially mutate.
    let bad = request(
        &addr,
        "POST",
        &format!("/session/{id}/window/rect"),
        Some(r#"{"width":-1}"#),
    );
    assert_eq!(bad["value"]["error"], json!("invalid argument"));
    let unchanged = request(&addr, "GET", &format!("/session/{id}/window/rect"), None);
    assert_eq!(unchanged["value"]["width"], json!(800));
}

#[test]
fn execute_sync_interrupts_infinite_loop() {
    let (addr, _fixture) = start(Vec::new());
    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    request(
        &addr,
        "POST",
        &format!("/session/{id}/timeouts"),
        Some(r#"{"script":200}"#),
    );
    let started = Instant::now();
    let timed_out = request(
        &addr,
        "POST",
        &format!("/session/{id}/execute/sync"),
        Some(r#"{"script":"while(true){}","args":[]}"#),
    );
    assert_eq!(timed_out["value"]["error"], json!("script timeout"));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "interrupt waited {:?}",
        started.elapsed()
    );
}

#[test]
fn unknown_element_click_and_perform_actions_are_unsupported() {
    let (addr, _fixture) = start(Vec::new());
    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    // Element ids are minted by any node returned to the client; an id that
    // was never issued fails the engine-side lookup as an unknown element.
    let click = request(
        &addr,
        "POST",
        &format!("/session/{id}/element/1/click"),
        Some("{}"),
    );
    assert_eq!(click["value"]["error"], json!("no such element"));
    let actions = request(&addr, "POST", &format!("/session/{id}/actions"), Some("{}"));
    assert_eq!(actions["value"]["error"], json!("unsupported operation"));
    let released = request(&addr, "DELETE", &format!("/session/{id}/actions"), None);
    assert_eq!(released["value"], Value::Null);
}

#[test]
fn take_screenshot_returns_base64_png() {
    let (addr, _fixture) = start(Vec::new());
    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let shot = request(&addr, "GET", &format!("/session/{id}/screenshot"), None);
    let data = shot["value"].as_str().expect("base64 string");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("base64 png");
    // PNG signature, then the IHDR dimensions (big-endian after the 8-byte
    // length/type prefix).
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("width"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("height"));
    assert_eq!((width, height), (800, 600), "unexpected viewport capture");
}
