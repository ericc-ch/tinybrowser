use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use browser::{Browser, Profile};
use serde_json::json;

fn temp_data_home() -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("tinybrowser-cdp-{stamp}"));
    std::fs::create_dir_all(&dir).expect("temp");
    dir
}

fn spawn_server(browser: browser::BrowserHandle) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let join = thread::spawn(move || {
        let _ = cdp::serve(&listener, &browser);
    });
    (addr, join)
}

#[test]
fn browser_target_page_runtime_flatten_and_method_not_found() {
    let data_home = temp_data_home();
    let browser = Browser::open_in(&data_home, &Profile::default());
    let (addr, _server) = spawn_server(browser.handle());
    thread::sleep(Duration::from_millis(20));
    let mut client = cdp::Client::connect(addr).expect("connect");

    let version = client
        .call("Browser.getVersion", &json!({}), None)
        .expect("version");
    assert_eq!(version["product"], json!("tinybrowser/0.1.0"));

    let created = client
        .call("Target.createTarget", &json!({"url": "about:blank"}), None)
        .expect("create");
    let target_id = created["targetId"].as_str().expect("target id").to_owned();

    let attached = client
        .call(
            "Target.attachToTarget",
            &json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .expect("attach");
    let session = attached["sessionId"].as_str().expect("session").to_owned();

    let evaluated = client
        .call(
            "Runtime.evaluate",
            &json!({"expression": "1 + 1"}),
            Some(&session),
        )
        .expect("eval");
    assert_eq!(evaluated["result"]["value"].as_f64(), Some(2.0));

    let missing = client.call("Foo.bar", &json!({}), None);
    let err = missing.expect_err("method-not-found");
    assert!(err.to_string().contains("wasn't found"), "{err}");

    client
        .call("Target.closeTarget", &json!({"targetId": target_id}), None)
        .expect("close");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn page_navigate_loads_http_document() {
    fn read_target(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        let mut head = Vec::new();
        let mut chunk = [0_u8; 512];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).expect("read");
            assert_ne!(read, 0);
            head.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8_lossy(&head)
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .expect("target")
            .to_owned()
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("page bind");
    let page_addr = listener.local_addr().expect("page addr");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_target(&mut stream);
        let body = b"<!doctype html><p id=ok>hi</p>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("head");
        stream.write_all(body).expect("body");
    });

    let data_home = temp_data_home();
    let browser = Browser::open_in(&data_home, &Profile::default());
    let (addr, _cdp) = spawn_server(browser.handle());
    thread::sleep(Duration::from_millis(20));
    let mut client = cdp::Client::connect(addr).expect("connect");
    let created = client
        .call("Target.createTarget", &json!({"url": "about:blank"}), None)
        .expect("create");
    let target_id = created["targetId"].as_str().expect("id").to_owned();
    let attached = client
        .call(
            "Target.attachToTarget",
            &json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .expect("attach");
    let session = attached["sessionId"].as_str().expect("session").to_owned();
    client
        .call(
            "Page.navigate",
            &json!({"url": format!("http://{page_addr}/")}),
            Some(&session),
        )
        .expect("navigate");
    let evaluated = client
        .call(
            "Runtime.evaluate",
            &json!({"expression": "document.getElementsByTagName('p')[0].firstChild.data"}),
            Some(&session),
        )
        .expect("eval");
    assert_eq!(evaluated["result"]["value"], json!("hi"));
    server.join().expect("page server");
    let _ = std::fs::remove_dir_all(data_home);
}

fn http_get(addr: std::net::SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("http connect");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("http write");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).expect("http read");
    let text = String::from_utf8_lossy(&buf);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .trim()
        .to_owned();
    (status, body)
}

#[test]
fn json_discovery_page_socket_close_target_and_browser_close() {
    let data_home = temp_data_home();
    let browser = Browser::open_in(&data_home, &Profile::default());
    let (addr, server) = spawn_server(browser.handle());
    thread::sleep(Duration::from_millis(20));

    let mut client = cdp::Client::connect(addr).expect("connect");
    let created = client
        .call("Target.createTarget", &json!({"url": "about:blank"}), None)
        .expect("create");
    let target_id = created["targetId"].as_str().expect("target id").to_owned();

    let (status, version) = http_get(addr, "/json/version");
    assert_eq!(status, 200);
    let version: serde_json::Value = serde_json::from_str(&version).expect("version json");
    assert_eq!(version["Browser"], json!("tinybrowser/0.1.0"));
    assert_eq!(
        version["webSocketDebuggerUrl"],
        json!(format!("ws://{addr}/devtools/browser"))
    );

    let (status, list) = http_get(addr, "/json/list");
    assert_eq!(status, 200);
    let list: serde_json::Value = serde_json::from_str(&list).expect("list json");
    let page_url = list[0]["webSocketDebuggerUrl"].as_str().expect("page ws");
    assert!(
        page_url.ends_with(&format!("/devtools/page/{target_id}")),
        "{page_url}"
    );

    let mut page_client =
        cdp::Client::connect_path(addr, &format!("/devtools/page/{target_id}")).expect("page ws");
    let evaluated = page_client
        .call("Runtime.evaluate", &json!({"expression": "1+1"}), None)
        .expect("page eval");
    assert_eq!(evaluated["result"]["value"].as_f64(), Some(2.0));

    let missing_page = cdp::Client::connect_path(addr, "/devtools/page/not-a-number");
    assert!(missing_page.is_err(), "invalid page path must not upgrade");
    let unknown_path = cdp::Client::connect_path(addr, "/foo");
    assert!(
        unknown_path.is_err(),
        "unknown upgrade path must not become browser"
    );

    let unflat = client.call(
        "Target.attachToTarget",
        &json!({"targetId": target_id}),
        None,
    );
    let err = unflat.expect_err("flatten required");
    assert!(err.to_string().contains("flatten"), "{err}");

    let attached = client
        .call(
            "Target.attachToTarget",
            &json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .expect("attach");
    let session = attached["sessionId"].as_str().expect("session").to_owned();
    client
        .call("Target.closeTarget", &json!({"targetId": target_id}), None)
        .expect("close target");
    let after_close = client.call(
        "Runtime.evaluate",
        &json!({"expression": "1"}),
        Some(&session),
    );
    let err = after_close.expect_err("closed session");
    assert!(
        err.to_string().contains("unknown session") || err.to_string().contains("stopped"),
        "{err}"
    );

    client
        .call("Browser.close", &json!({}), None)
        .expect("browser close");
    let after_browser = client.call("Browser.getVersion", &json!({}), None);
    assert!(after_browser.is_err(), "getVersion after Browser.close");
    server.join().expect("serve returns after Browser.close");
    let _ = std::fs::remove_dir_all(data_home);
}
