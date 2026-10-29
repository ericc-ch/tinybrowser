//! Shared fixtures for the browser-level integration suites in this binary.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

/// Reads one HTTP/1.1 request head and returns its request target, discarding
/// the rest. Test servers call this before replying: closing a socket with
/// unread data sends RST, which can abort the response delivery. `None` means
/// the connection failed or the request line was malformed; the server loop
/// moves on to the next connection.
pub(crate) fn read_request_path(stream: &mut TcpStream) -> Option<String> {
    // Only the request line matters, so decode up to its line break and leave
    // the (possibly non-UTF-8) header bytes alone.
    let head = read_request_head_bytes(stream)?;
    let line = match head.windows(2).position(|window| window == b"\r\n") {
        Some(end) => &head[..end],
        None => &head[..],
    };
    let line = String::from_utf8_lossy(line);
    line.split_whitespace().nth(1).map(str::to_owned)
}

/// Reads one request head, including its blank line, and returns it as text so
/// a test can inspect the headers the client sent.
pub(crate) fn read_request_head(stream: &mut TcpStream) -> Option<String> {
    let head = read_request_head_bytes(stream)?;
    Some(String::from_utf8_lossy(&head).into_owned())
}

fn read_request_head_bytes(stream: &mut TcpStream) -> Option<Vec<u8>> {
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
    Some(head)
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

/// One 1x1 red PNG.
pub(crate) const PIXEL: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc, 0xcf, 0xc0, 0x50,
    0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// One request a [`TestServer`] accepted: its target and its whole head.
pub(crate) struct RecordedRequest {
    pub(crate) path: String,
    pub(crate) head: String,
}

/// A route table plus a recorder: serves the response whose path matches,
/// records every request head, and holds one path until [`Self::release`].
/// Each connection runs on its own thread, so a held response does not block
/// the rest. A response echoes the request's `Origin` and allows credentials,
/// so a CORS subresource completes; a request without one gets `*`.
pub(crate) struct TestServer {
    address: std::net::SocketAddr,
    receiver: Mutex<mpsc::Receiver<RecordedRequest>>,
    history: Mutex<Vec<RecordedRequest>>,
    released: Arc<(Mutex<bool>, Condvar)>,
}

impl TestServer {
    pub(crate) fn start(
        routes: Vec<(&'static str, &'static str, &'static [u8])>,
        hold: Option<&'static str>,
    ) -> Self {
        Self::with_cors(routes, hold, true)
    }

    /// A server that sends no CORS headers, so a `cors`-mode request to it
    /// fails the CORS check
    /// (<https://fetch.spec.whatwg.org/#cors-check>).
    pub(crate) fn start_without_cors(
        routes: Vec<(&'static str, &'static str, &'static [u8])>,
    ) -> Self {
        Self::with_cors(routes, None, false)
    }

    fn with_cors(
        routes: Vec<(&'static str, &'static str, &'static [u8])>,
        hold: Option<&'static str>,
        cors: bool,
    ) -> Self {
        let (listener, address) = server();
        let (sender, receiver) = mpsc::channel();
        let released: Arc<(Mutex<bool>, Condvar)> = Arc::default();
        let flag = Arc::clone(&released);
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let sender = sender.clone();
                let flag = Arc::clone(&flag);
                let routes = routes.clone();
                thread::spawn(move || {
                    let Some(head) = read_request_head(&mut stream) else {
                        return;
                    };
                    let path = request_path(&head);
                    let _ = sender.send(RecordedRequest {
                        path: path.clone(),
                        head: head.clone(),
                    });
                    if hold == Some(path.as_str()) {
                        let (lock, condvar) = &*flag;
                        let mut released = lock.lock().expect("release flag");
                        while !*released {
                            released = condvar.wait(released).expect("release wait");
                        }
                    }
                    let (content_type, body) = routes
                        .iter()
                        .find(|(route, _, _)| *route == path)
                        .map_or(("text/plain", &[][..]), |(_, kind, body)| (*kind, *body));
                    let response = response_for(&head, content_type, body, cors);
                    // The client may hang up right after its head; a failed
                    // write then needs no retry.
                    let _ = stream.write_all(&response);
                });
            }
        });
        Self {
            address,
            receiver: Mutex::new(receiver),
            history: Mutex::new(Vec::new()),
            released,
        }
    }

    pub(crate) fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    /// Waits for a request whose target is `path` and returns its head.
    pub(crate) fn wait(&self, path: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(found) = self
                .history
                .lock()
                .expect("request history")
                .iter()
                .rev()
                .find(|request| request.path == path)
            {
                return found.head.clone();
            }
            match self
                .receiver
                .lock()
                .expect("request channel")
                .recv_timeout(Duration::from_millis(100))
            {
                Ok(recorded) => self.history.lock().expect("request history").push(recorded),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => panic!("request recorder stopped"),
            }
            assert!(Instant::now() < deadline, "no request for {path}");
        }
    }

    /// Whether any request for `path` has been accepted.
    pub(crate) fn requested(&self, path: &str) -> bool {
        self.drain();
        self.history
            .lock()
            .expect("request history")
            .iter()
            .any(|request| request.path == path)
    }

    /// Waits until at least `count` requests for `path` have been accepted.
    pub(crate) fn wait_count(&self, path: &str, count: usize) -> Vec<String> {
        poll_until(&format!("{count} requests for {path}"), || {
            let heads = self.history(path);
            (heads.len() >= count).then_some(heads)
        })
    }

    /// Every recorded head for `path`, oldest first.
    pub(crate) fn history(&self, path: &str) -> Vec<String> {
        self.drain();
        self.history
            .lock()
            .expect("request history")
            .iter()
            .filter(|request| request.path == path)
            .map(|request| request.head.clone())
            .collect()
    }

    fn drain(&self) {
        let receiver = self.receiver.lock().expect("request channel");
        while let Ok(recorded) = receiver.try_recv() {
            self.history.lock().expect("request history").push(recorded);
        }
    }

    pub(crate) fn release(&self) {
        let (lock, condvar) = &*self.released;
        *lock.lock().expect("release flag") = true;
        condvar.notify_all();
    }
}

/// Polls `probe` until it answers, failing the test after ten seconds.
pub(crate) fn poll_until<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(answer) = probe() {
            return answer;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The request target of a recorded head.
fn request_path(head: &str) -> String {
    head.split_whitespace().nth(1).unwrap_or("/").to_owned()
}

/// One response, echoing the request's `Origin` when it sent one so a
/// credentialed CORS request passes the CORS check.
fn response_for(head: &str, content_type: &str, body: &[u8], cors: bool) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if !cors {
        response.push_str("\r\n");
        let mut response = response.into_bytes();
        response.extend_from_slice(body);
        return response;
    }
    match request_header(head, "origin") {
        Some(origin) => {
            let _ = write!(
                response,
                "Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Credentials: true\r\nVary: Origin\r\n"
            );
        }
        None => response.push_str("Access-Control-Allow-Origin: *\r\n"),
    }
    response.push_str("\r\n");
    let mut response = response.into_bytes();
    response.extend_from_slice(body);
    response
}

/// One request header's value, case-insensitively.
fn request_header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (header, value) = line.split_once(':')?;
        header.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Serves one body at `/` on an ephemeral port for every connection it gets./// Serves one body at `/` on an ephemeral port for every connection it gets.
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
