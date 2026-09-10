use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use browser::{AgentBuilder, Browser, NetworkSession, Profile, ProfileStore, Renderers};
use serde_json::{Value, json};

fn start(builder: AgentBuilder) -> (String, Browser) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let browser = Browser::open_with_network_and(
        NetworkSession::from_builder(builder, ProfileStore::memory(&Profile::default()))
            .expect("network"),
        Renderers::Local,
    );
    let handle = browser.handle();
    thread::spawn(move || {
        let _ = webdriver::serve(&listener, handle);
    });
    (addr, browser)
}

fn request(addr: &str, method: &str, path: &str, body: Option<&str>) -> Value {
    let payload = body.unwrap_or("");
    let mut last_error = None;
    let deadline = Instant::now() + Duration::from_secs(2);
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
    let (addr, _browser) = start(AgentBuilder::new());

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
}

#[test]
fn execute_sync_waits_for_returned_promise_or_script_timeout() {
    let (addr, _browser) = start(AgentBuilder::new());

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
    let page_listener = TcpListener::bind("127.0.0.1:0").expect("page bind");
    let page_addr = page_listener.local_addr().expect("page addr");
    let server = thread::spawn(move || {
        let (mut navigation, _) = page_listener.accept().expect("navigation");
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

    let (addr, _browser) = start(AgentBuilder::new());

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
        Some(&format!(r#"{{"url":"http://{page_addr}/"}}"#)),
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
    let page_listener = TcpListener::bind("127.0.0.1:0").expect("page bind");
    let page_addr = page_listener.local_addr().expect("page addr");
    let server = thread::spawn(move || {
        let (mut navigation, _) = page_listener.accept().expect("navigation");
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

    let builder = AgentBuilder::new()
        .resolve("*.test=127.0.0.1")
        .expect("resolve map");
    let (addr, _browser) = start(builder);

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
            page_addr.port()
        )),
    );
    assert_eq!(navigated["value"], json!(null));

    let current = request(&addr, "GET", &format!("/session/{id}/url"), None);
    assert_eq!(
        current["value"].as_str().expect("url"),
        format!("http://web-platform.test:{}/", page_addr.port())
    );
    server.join().expect("server");
}

#[test]
fn one_session_delete_leaves_pages_close_last_window_invalidates() {
    let (addr, browser) = start(AgentBuilder::new());

    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    assert_eq!(browser.handle().pages().len(), 1);

    let second = request(&addr, "POST", "/session", Some("{}"));
    assert_eq!(second["value"]["error"], json!("session not created"));
    assert_eq!(browser.handle().pages().len(), 1);

    request(&addr, "DELETE", &format!("/session/{id}"), None);
    // ADR 0009: product DELETE /session detaches automation and leaves tabs.
    assert_eq!(browser.handle().pages().len(), 1);
    let gone = request(&addr, "GET", &format!("/session/{id}/window"), None);
    assert_eq!(gone["value"]["error"], json!("invalid session id"));

    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    assert_eq!(browser.handle().pages().len(), 2);

    let closed = request(&addr, "DELETE", &format!("/session/{id}/window"), None);
    assert_eq!(closed["value"], json!([]));
    assert_eq!(browser.handle().pages().len(), 1);
    let invalid = request(&addr, "GET", &format!("/session/{id}/window"), None);
    assert_eq!(invalid["value"]["error"], json!("invalid session id"));
}

#[test]
fn execute_sync_interrupts_infinite_loop() {
    let (addr, _browser) = start(AgentBuilder::new());
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
fn click_and_actions_are_unsupported() {
    let (addr, _browser) = start(AgentBuilder::new());
    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let click = request(
        &addr,
        "POST",
        &format!("/session/{id}/element/1/click"),
        Some("{}"),
    );
    assert_eq!(click["value"]["error"], json!("unsupported operation"));
    let actions = request(&addr, "POST", &format!("/session/{id}/actions"), Some("{}"));
    assert_eq!(actions["value"]["error"], json!("unsupported operation"));
    let released = request(&addr, "DELETE", &format!("/session/{id}/actions"), None);
    assert_eq!(released["value"]["error"], json!("unsupported operation"));
}
