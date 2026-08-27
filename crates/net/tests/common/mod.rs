//! Recording-fake loopback server shared by the net integration tests.
#![allow(dead_code)]
//!
//! Per decision 10: assertions check captured requests, never spies or call
//! counts. The server is a plain TCP listener speaking canned HTTP; every
//! connection's request head is recorded verbatim off the wire so tests can
//! assert on exactly what the backend put on the socket.
//!
//! Each [`TestServer`] binds `127.0.0.1:0` (kernel-assigned port) and runs
//! one accept-loop thread until dropped. The workspace sets
//! `RUST_TEST_THREADS=1`, so servers never contend.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a test server waits for the peer to close before giving up.
const PEER_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// One request captured verbatim off the wire.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    /// Request-line method token, e.g. `"GET"`.
    pub method: String,
    /// Request-line target as sent, e.g. `"/a/b?c=d"`.
    pub target: String,
    /// Request-line version, e.g. `"HTTP/1.1"`.
    pub version: String,
    /// Header lines in wire order, name and value verbatim (case included).
    pub headers: Vec<(String, String)>,
    /// Body bytes as declared by `Content-Length` (empty when absent).
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// Value of the first header whose ASCII-case-insensitive name matches.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One live server-side connection handed to the test's handler closure.
pub struct Connection {
    stream: TcpStream,
    recorder: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl Connection {
    /// Read one full request head plus any `Content-Length` body.
    ///
    /// Blocking with a generous per-read timeout so a misbehaving client
    /// fails the test instead of hanging the single test thread forever.
    ///
    /// # Panics
    ///
    /// Panics if the peer closes mid-exchange, sends a malformed head, or
    /// the head exceeds 64 KiB — all mean the test itself wrote a broken
    /// scenario.
    pub fn read_request(&mut self) -> RecordedRequest {
        self.stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout is settable");
        let mut buf = Vec::new();
        let head_end = loop {
            if let Some(pos) = find_head_end(&buf) {
                break pos;
            }
            assert!(buf.len() <= 64 * 1024, "request head exceeded 64 KiB");
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).expect("peer readable");
            assert_ne!(n, 0, "peer closed mid-request-head");
            buf.extend_from_slice(&chunk[..n]);
        };

        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default().to_owned();
        let mut parts = request_line.splitn(3, ' ');
        let method = parts.next().unwrap_or_default().to_owned();
        let target = parts.next().unwrap_or_default().to_owned();
        let version = parts.next().unwrap_or_default().to_owned();

        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let Some((name, value)) = line.split_once(':') else {
                panic!("malformed header line in canned capture: {line:?}");
            };
            headers.push((name.to_owned(), value.trim_start_matches(' ').to_owned()));
        }

        let content_length = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = buf[head_end + 4..].to_vec();
        while body.len() < content_length {
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).expect("peer readable");
            assert_ne!(n, 0, "peer closed mid-request-body");
            body.extend_from_slice(&chunk[..n]);
        }
        body.truncate(content_length);

        let request = RecordedRequest {
            method,
            target,
            version,
            headers,
            body,
        };
        self.recorder
            .lock()
            .expect("recorder poisoned")
            .push(request.clone());
        request
    }

    /// Write raw bytes to the wire and flush.
    pub fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()
    }

    /// The live TCP stream, for upgrades (WebSocket handshake) that take
    /// over the socket after the request head.
    pub fn stream_mut(&mut self) -> &mut TcpStream {
        &mut self.stream
    }

    /// Wait until the peer half-closes or resets the connection.
    ///
    /// Returns `true` only when an EOF or error was observed within
    /// [`PEER_CLOSE_TIMEOUT`] — i.e. the client actually closed its side.
    #[must_use]
    pub fn await_peer_close(&mut self) -> bool {
        let deadline = Instant::now() + PEER_CLOSE_TIMEOUT;
        self.stream
            .set_read_timeout(Some(Duration::from_millis(25)))
            .expect("read timeout is settable");
        while Instant::now() < deadline {
            match self.stream.read(&mut [0u8; 1]) {
                // EOF: the client closed cleanly.
                Ok(0) => return true,
                // The client must not send more data after dropping; drain
                // anything unexpected but keep waiting for closure.
                Ok(_) => {}
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                // ECONNRESET and friends: an abrupt close still proves the
                // client tore down its side.
                Err(_) => return true,
            }
        }
        false
    }
}

/// Position of the `\r\n\r\n` that ends a request head, if complete.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// A loopback HTTP server recording everything it receives.
///
/// Drop shuts the listener down and joins the accept thread. Handler
/// panics do not vanish with the thread: they are captured and surface
/// through [`TestServer::assert_clean`], which every test calls before
/// finishing.
pub struct TestServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    failures: Arc<Mutex<Vec<String>>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl TestServer {
    /// Start a server whose per-connection behavior is `handler`.
    ///
    /// Every accepted connection invokes `handler` once with the live
    /// stream wrapper; the handler reads the request, writes whatever
    /// canned bytes the scenario needs, and may observe peer close.
    pub fn start(handler: impl Fn(&mut Connection) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind succeeds");
        let addr = listener.local_addr().expect("local addr known");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);

        let listener_clone = listener;
        let requests_clone = Arc::clone(&requests);
        let failures_clone = Arc::clone(&failures);
        let shutdown_clone = Arc::clone(&shutdown);
        let handle = std::thread::spawn(move || {
            loop {
                if shutdown_clone.load(Ordering::Acquire) {
                    break;
                }
                match listener_clone.accept() {
                    Ok((stream, _)) => {
                        // Teardown probe: `Drop` sets the flag BEFORE
                        // dialing one dummy connection just to unblock this
                        // accept. Route it around the handler — handing it
                        // over would manufacture a spurious "peer closed
                        // mid-request-head" panic in the failures list on
                        // every server teardown.
                        if shutdown_clone.load(Ordering::Acquire) {
                            drop(stream);
                            break;
                        }
                        let mut conn = Connection {
                            stream,
                            recorder: Arc::clone(&requests_clone),
                        };
                        // A panicking handler is a failed test, not noise:
                        // capture so the main thread can assert on it.
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                handler(&mut conn);
                            }));
                        if let Err(panic) = outcome {
                            let message = panic
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                                .unwrap_or_else(|| "handler panicked".to_owned());
                            failures_clone
                                .lock()
                                .expect("failures poisoned")
                                .push(message);
                        }
                    }
                    Err(err) => {
                        // Loud, not silent: an EMFILE-style condition must
                        // fail a visible assert (via `assert_clean`) rather
                        // than quietly end the server and confuse whatever
                        // dials next.
                        failures_clone
                            .lock()
                            .expect("failures poisoned")
                            .push(format!("accept failed: {err}"));
                        break;
                    }
                }
            }
        });

        Self {
            addr,
            requests,
            failures,
            shutdown,
            handle: Some(handle),
        }
    }

    /// Absolute URL for `path` on this server's loopback address.
    #[must_use]
    pub fn url(&self, path: &str) -> url::Url {
        url::Url::parse(&format!("http://{}{path}", self.addr)).expect("server URL is absolute")
    }

    /// Absolute `ws://` URL for `path` on this server's loopback address.
    #[must_use]
    pub fn ws_url(&self, path: &str) -> url::Url {
        url::Url::parse(&format!("ws://{}{path}", self.addr)).expect("server URL is absolute")
    }

    /// All requests recorded so far, in arrival order.
    #[must_use]
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("recorder poisoned").clone()
    }

    /// The address the server is listening on.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Fail the test if any handler panicked.
    ///
    /// # Panics
    ///
    /// Panics with the captured message when a handler did.
    pub fn assert_clean(&self) {
        let failures = self.failures.lock().expect("failures poisoned");
        assert!(failures.is_empty(), "server-side failures: {failures:?}");
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // Unblock the pending accept by connecting once more; best effort.
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
