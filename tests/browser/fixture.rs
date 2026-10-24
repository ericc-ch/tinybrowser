//! Shared fixtures for the browser-level integration suites in this binary.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

/// Reads one HTTP/1.1 request head and discards it. Test servers call this
/// before replying: closing a socket with unread data sends RST, which can
/// abort the response delivery.
pub(crate) fn read_request_head(stream: &mut TcpStream) {
    // Short polls instead of one long blocking read: under load the request
    // can lag the accept, and a single long read hides which side stalled.
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("read timeout");
    let mut head = Vec::new();
    let mut chunk = [0_u8; 512];
    let deadline = Instant::now() + Duration::from_secs(10);
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        assert!(Instant::now() < deadline, "no request head within 10s");
        match stream.read(&mut chunk) {
            Ok(0) => panic!("request closed early"),
            Ok(n) => head.extend_from_slice(&chunk[..n]),
            // A poll timeout surfaces as `WouldBlock` or `TimedOut` depending
            // on the platform.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("read: {error}"),
        }
    }
}

/// Serves one HTML body on an ephemeral port for every connection it gets,
/// reading each request head first. The thread runs until the test process
/// exits; a test that only loads the page once simply never sees a second
/// connection.
pub(crate) fn serve_html(body: &'static [u8]) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("server bind");
    let addr = listener.local_addr().expect("server addr");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n",
        body.len()
    );
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            read_request_head(&mut stream);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    addr
}

/// Creates a target, attaches to it, and waits for `Page.loadEventFired`
/// after navigating it to `url`; returns the attached session id.
pub(crate) fn load_page(client: &mut cdp::Client, url: &str, timeout: Duration) -> String {
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
        .call("Page.enable", &json!({}), Some(&session))
        .expect("enable Page events");
    client
        .call("Page.navigate", &json!({"url": url}), Some(&session))
        .expect("navigate");
    wait_for_load(client, timeout);
    session
}

/// Waits for the next `Page.loadEventFired` event.
pub(crate) fn wait_for_load(client: &mut cdp::Client, timeout: Duration) -> serde_json::Value {
    let deadline = Instant::now() + timeout;
    loop {
        let event = client
            .read_event(Duration::from_millis(500))
            .expect("read event")
            .expect("load event");
        if event["method"] == json!("Page.loadEventFired") {
            return event;
        }
        assert!(Instant::now() < deadline, "no load event: {event}");
    }
}

pub struct Fixture {
    pub runtime: PathBuf,
    pub data: PathBuf,
    pub children: Vec<Child>,
}

impl Fixture {
    pub fn new(prefix: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("{prefix}-{stamp}"));
        let runtime = root.join("run");
        let data = root.join("data");
        std::fs::create_dir_all(&runtime).expect("runtime");
        std::fs::create_dir_all(&data).expect("data");
        Self {
            runtime,
            data,
            children: Vec::new(),
        }
    }

    pub fn spawn_daemon(&mut self) -> u32 {
        let child = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
            .args(["daemon", "--profile=default"])
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("XDG_DATA_HOME", &self.data)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon");
        let id = child.id();
        self.children.push(child);
        id
    }

    pub fn wait_json(&self) -> serde_json::Value {
        let path = self.runtime.join("tinybrowser/default/daemon.json");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            {
                return value;
            }
            assert!(Instant::now() < deadline, "daemon.json missing");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn endpoint(&self) -> std::net::SocketAddr {
        let info = self.wait_json();
        let port = u16::try_from(info["port"].as_u64().expect("port")).expect("port");
        format!("127.0.0.1:{port}").parse().expect("addr")
    }

    pub fn connect(&self) -> cdp::Client {
        cdp::Client::connect(self.endpoint()).expect("cdp connect")
    }

    pub fn spawn_daemon_with_env(&mut self, key: &str, value: &str) -> u32 {
        let child = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
            .args(["daemon", "--profile=default"])
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("XDG_DATA_HOME", &self.data)
            .env(key, value)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon");
        let id = child.id();
        self.children.push(child);
        id
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Ok(text) =
            std::fs::read_to_string(self.runtime.join("tinybrowser/default/daemon.json"))
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(pid) = value["pid"].as_u64()
        {
            let _ = Command::new("kill")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if let Some(root) = self.runtime.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
