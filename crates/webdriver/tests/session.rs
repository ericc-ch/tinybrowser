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
        let _ = webdriver::serve(&listener);
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
}
