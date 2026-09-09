use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

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
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        let _ = webdriver::serve(&listener, webdriver::AgentBuilder::new());
    });

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

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        let _ = webdriver::serve(&listener, webdriver::AgentBuilder::new());
    });

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

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        let builder = webdriver::AgentBuilder::new()
            .resolve("*.test=127.0.0.1")
            .expect("resolve map");
        let _ = webdriver::serve(&listener, builder);
    });

    let created = request(&addr, "POST", "/session", Some("{}"));
    let id = created["value"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();

    let opened = request(&addr, "POST", &format!("/session/{id}/window/new"), Some("{}"));
    let handle = opened["value"]["handle"].as_str().expect("handle").to_owned();
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
