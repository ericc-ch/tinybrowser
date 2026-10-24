//! Shared fixtures for the browser-level integration suites in this binary.

use std::io::Read;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Reads one HTTP/1.1 request head and returns the request target. Test
/// servers call this before replying: closing a socket with unread data sends
/// RST, which can abort the response delivery.
pub(crate) fn read_request_target(stream: &mut TcpStream) -> String {
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
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("read: {error}"),
        }
    }
    String::from_utf8_lossy(&head)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("target")
        .to_owned()
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
