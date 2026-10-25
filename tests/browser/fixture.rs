//! Shared fixtures for the browser-level integration suites in this binary.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

/// Reads one HTTP/1.1 request head and returns its request target, discarding
/// the rest. Test servers call this before replying: closing a socket with
/// unread data sends RST, which can abort the response delivery. `None` means
/// the connection failed or the request line was malformed; the server loop
/// moves on to the next connection.
pub(crate) fn read_request_path(stream: &mut TcpStream) -> Option<String> {
    // Short polls instead of one long blocking read: under load the request
    // can lag the accept, and a single long read hides which side stalled.
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .ok()?;
    let mut head = Vec::new();
    let mut chunk = [0_u8; 512];
    let deadline = Instant::now() + Duration::from_secs(10);
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if Instant::now() >= deadline {
            return None;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => head.extend_from_slice(&chunk[..n]),
            // A poll timeout surfaces as `WouldBlock` or `TimedOut` depending
            // on the platform.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                eprintln!("test server read failed: {error}");
                return None;
            }
        }
    }
    // Only the request line matters, so decode up to its line break and leave
    // the (possibly non-UTF-8) header bytes alone.
    let line = match head.windows(2).position(|window| window == b"\r\n") {
        Some(end) => &head[..end],
        None => &head[..],
    };
    let line = String::from_utf8_lossy(line);
    line.split_whitespace().nth(1).map(str::to_owned)
}

/// Binds an ephemeral loopback port for a test server. The caller learns the
/// address before starting the server, so a page it serves can name its own
/// origin.
pub(crate) fn server() -> (TcpListener, std::net::SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("server bind");
    let address = listener.local_addr().expect("server addr");
    (listener, address)
}

/// Serves one response per request path for every connection the listener
/// gets, reading each request head first. An unknown path gets an empty 404.
/// The thread runs until the test process exits.
pub(crate) fn serve_on(listener: TcpListener, routes: Vec<(&'static str, Vec<u8>)>) {
    serve_on_after(listener, routes, Duration::ZERO);
}

/// [`serve_on`] with a delay before each response, so a test can observe a
/// request that is still in flight.
pub(crate) fn serve_on_after(
    listener: TcpListener,
    routes: Vec<(&'static str, Vec<u8>)>,
    delay: Duration,
) {
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
            thread::sleep(delay);
            let _ = stream.write_all(&response);
        }
    });
}

/// A `200` response with `body` and `content_type`.
pub(crate) fn ok(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// A `302` response to `location`.
pub(crate) fn redirect(location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

/// Serves one body at `/` on an ephemeral port for every connection it gets.
pub(crate) fn serve(body: &'static [u8], content_type: &'static str) -> std::net::SocketAddr {
    let (listener, address) = server();
    serve_on(listener, vec![("/", ok(content_type, body))]);
    address
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
    wait_for_load(client, &session, timeout);
    session
}

/// Waits for the next `Page.loadEventFired` event for `session`. An event read
/// that times out is retried until the overall deadline.
pub(crate) fn wait_for_load(client: &mut cdp::Client, session: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let event = client
            .read_event(Duration::from_millis(500))
            .expect("read event");
        if let Some(event) = event
            && event["method"] == json!("Page.loadEventFired")
            && event["sessionId"] == json!(session)
        {
            return;
        }
        assert!(Instant::now() < deadline, "no load event for {session}");
    }
}

/// Evaluates `expression` in `session`, awaiting a promise and returning the
/// value through the protocol's JSON projection.
pub(crate) fn evaluate(
    client: &mut cdp::Client,
    session: &str,
    expression: &str,
) -> serde_json::Value {
    client
        .call(
            "Runtime.evaluate",
            &json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
            Some(session),
        )
        .expect("evaluate")["result"]["value"]
        .clone()
}

/// Starts the daemon, loads `url` in a new target, and returns the fixture
/// that owns the daemon plus the attached session.
pub(crate) fn open_page(prefix: &str, url: &str) -> (Fixture, cdp::Client, String) {
    let mut fixture = Fixture::new(prefix);
    fixture.spawn_daemon();
    fixture.wait_json();
    let mut client = fixture.connect();
    let session = load_page(&mut client, url, Duration::from_secs(10));
    (fixture, client, session)
}

pub struct Fixture {
    runtime: PathBuf,
    data: PathBuf,
    children: Vec<Child>,
}

impl Fixture {
    /// The profile's runtime directory.
    pub fn runtime(&self) -> &std::path::Path {
        &self.runtime
    }

    /// The profile's data directory.
    pub fn data(&self) -> &std::path::Path {
        &self.data
    }

    /// Live daemon children, in spawn order.
    pub fn children(&mut self) -> &mut [Child] {
        &mut self.children
    }

    /// Adopts a child the test spawned itself.
    pub fn append_child(&mut self, child: Child) {
        self.children.push(child);
    }

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
        self.spawn_daemon_with(&[])
    }

    pub fn spawn_daemon_with_env(&mut self, key: &str, value: &str) -> u32 {
        self.spawn_daemon_with(&[(key, value)])
    }

    pub fn spawn_daemon_with(&mut self, env: &[(&str, &str)]) -> u32 {
        let child = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
            .args(["daemon", "--profile=default"])
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("XDG_DATA_HOME", &self.data)
            .envs(env.iter().copied())
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
