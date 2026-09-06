mod common;

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::TestServer;
use net::{
    Agent, AgentBuilder, LimitExceeded, Method, NetError, ProtocolError, TimeoutKind,
    TransportError,
};

const OBSERVE_TIMEOUT: Duration = Duration::from_secs(5);
const PROXY_PROBE_FLAG: &str = "NET_CRATE_PROXY_PROBE";

fn canned_ok(headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = b"HTTP/1.1 200 OK\r\n".to_vec();
    for (name, value) in headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(body);
    out
}

fn canned_redirect(status: u16, location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

fn await_flag(flag: &AtomicBool) {
    let deadline = Instant::now() + OBSERVE_TIMEOUT;
    while !flag.load(Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "server never observed the client"
        );
        std::thread::yield_now();
    }
}

fn assert_statuses_and_bodies() {
    for status in [201_u16, 302, 404, 500] {
        let server = TestServer::start(move |connection| {
            connection.read_request();
            connection
                .write_all(
                    format!("HTTP/1.1 {status} Whatever\r\nContent-Length: 2\r\n\r\nno").as_bytes(),
                )
                .expect("status response");
        });
        let response = Agent::new()
            .request(Method::GET, server.url("/"))
            .send()
            .expect("status is response data");
        assert_eq!(response.status(), status);
        assert_eq!(response.into_body().bytes(16).expect("body"), b"no");
        server.assert_clean();
    }
}

#[test]
fn http_transcripts_cover_status_headers_framing_limits_and_failures() {
    assert_statuses_and_bodies();

    let server = TestServer::start(|connection| {
        let request = connection.read_request();
        assert_eq!(request.method, "GET");
        assert_eq!(request.target, "/a/b?c=d");
        connection
            .write_all(&canned_ok(
                &[("Content-Type", "text/plain"), ("X-Mixed-Case", "Value")],
                b"hello",
            ))
            .expect("response");
    });
    let requested = server.url("/a/b?c=d");
    let response = Agent::new()
        .request(Method::GET, requested.clone())
        .send()
        .expect("GET");
    assert_eq!(response.status(), 200);
    assert_eq!(response.final_url(), &requested);
    assert_eq!(
        response.headers().get("CONTENT-TYPE"),
        Some(&b"text/plain"[..])
    );
    assert_eq!(response.into_body().bytes(16).expect("body"), b"hello");
    server.assert_clean();

    let server = TestServer::start(|connection| {
        connection.read_request();
        connection
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
            )
            .expect("chunked response");
    });
    assert_eq!(
        Agent::new()
            .request(Method::GET, server.url("/chunked"))
            .send()
            .expect("chunked")
            .into_body()
            .bytes(64)
            .expect("body"),
        b"hello world"
    );
    server.assert_clean();

    let payload = vec![b'x'; 32];
    let server = TestServer::start(move |connection| {
        connection.read_request();
        connection
            .write_all(&canned_ok(&[], &payload))
            .expect("limited response");
    });
    assert!(matches!(
        Agent::new()
            .request(Method::GET, server.url("/limit"))
            .send()
            .expect("response")
            .into_body()
            .bytes(16),
        Err(NetError::Limit(LimitExceeded::Size(16)))
    ));
    server.assert_clean();

    let dead = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let dead_url = format!("http://{}/", dead.local_addr().expect("address"));
    drop(dead);
    assert!(matches!(
        Agent::new()
            .request(Method::GET, url::Url::parse(&dead_url).expect("url"))
            .send(),
        Err(NetError::Transport(_))
    ));

    let server = TestServer::start(|connection| {
        connection.read_request();
        std::thread::sleep(Duration::from_millis(300));
    });
    assert!(matches!(
        AgentBuilder::new()
            .timeout_global(Duration::from_millis(60))
            .build()
            .request(Method::GET, server.url("/stall"))
            .send(),
        Err(NetError::Transport(TransportError::Timeout(
            TimeoutKind::Global
        )))
    ));
    server.assert_clean();
}

#[test]
fn response_bodies_stream_and_drop_cancels_the_socket() {
    let first_chunk_delivered = Arc::new(AtomicBool::new(false));
    let server_flag = Arc::clone(&first_chunk_delivered);
    let first = vec![b'a'; 1024];
    let rest = vec![b'b'; 99 * 1024];
    let server = TestServer::start(move |connection| {
        connection.read_request();
        connection
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    first.len() + rest.len()
                )
                .as_bytes(),
            )
            .expect("head");
        connection.write_all(&first).expect("first chunk");
        let deadline = Instant::now() + OBSERVE_TIMEOUT;
        while !server_flag.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "body was buffered until completion"
            );
            std::thread::yield_now();
        }
        connection.write_all(&rest).expect("remaining body");
    });
    let mut body = Agent::new()
        .request(Method::GET, server.url("/stream"))
        .send()
        .expect("stream")
        .into_body();
    let mut collected = Vec::new();
    let mut chunks = 0;
    while let Some(chunk) = body.read_chunk().expect("chunk") {
        collected.extend_from_slice(&chunk);
        chunks += 1;
        first_chunk_delivered.store(true, Ordering::Release);
    }
    assert_eq!(
        collected,
        [vec![b'a'; 1024], vec![b'b'; 99 * 1024]].concat()
    );
    assert!(chunks > 1);
    server.assert_clean();

    let peer_closed = Arc::new(AtomicBool::new(false));
    let server_flag = Arc::clone(&peer_closed);
    let server = TestServer::start(move |connection| {
        connection.read_request();
        connection
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 65536\r\n\r\npartial")
            .expect("partial body");
        server_flag.store(connection.await_peer_close(), Ordering::Release);
    });
    {
        let mut body = Agent::new()
            .request(Method::GET, server.url("/cancel"))
            .send()
            .expect("cancel")
            .into_body();
        let _ = body.read_chunk().expect("partial chunk");
    }
    await_flag(&peer_closed);
    server.assert_clean();
}

#[test]
fn request_shaping_methods_fragments_and_rejections_are_wire_visible() {
    let server = TestServer::start(|connection| {
        connection.read_request();
        connection
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("method response");
    });
    let expected = ["GET", "HEAD", "PATCH", "patch", "propfind", "eGg"];
    for token in expected {
        Agent::new()
            .request(Method::parse(token).expect("method"), server.url("/m"))
            .send()
            .expect("method request");
    }
    let requests = server.requests();
    let methods: Vec<_> = requests
        .iter()
        .map(|request| request.method.as_str())
        .collect();
    assert_eq!(methods, expected);
    server.assert_clean();

    let server = TestServer::start(|connection| {
        connection.read_request();
        connection
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("request response");
    });
    AgentBuilder::new()
        .user_agent("builder/default")
        .build()
        .request(Method::POST, server.url("/shape#fragment"))
        .header("X-Custom", "alpha")
        .expect("header")
        .header("User-Agent", "request/wins")
        .expect("header")
        .body(b"name=value")
        .send()
        .expect("shaped request");
    let request = &server.requests()[0];
    assert_eq!(request.target, "/shape");
    assert_eq!(request.body, b"name=value");
    assert_eq!(request.header("user-agent"), Some("request/wins"));
    assert_eq!(request.header("content-length"), Some("10"));
    server.assert_clean();

    let server = TestServer::start(|connection| {
        connection.read_request();
        connection
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("response");
    });
    let result = Agent::new()
        .request(Method::GET, server.url("/invalid"))
        .header("X-Bad", "line\r\nInjected: yes");
    assert!(matches!(result, Err(net::HeaderError::InvalidValue(_))));
    assert!(server.requests().is_empty());
    server.assert_clean();
}

#[test]
fn redirect_policy_covers_following_rewriting_caps_and_origin_safety() {
    for (status, method, expected_method, expected_body) in [
        (301, Method::POST, "GET", b"".as_slice()),
        (302, Method::POST, "GET", b"".as_slice()),
        (303, Method::PUT, "GET", b"".as_slice()),
        (307, Method::POST, "POST", b"field=1".as_slice()),
        (308, Method::POST, "POST", b"field=1".as_slice()),
    ] {
        let initial_method = method.as_str().to_owned();
        let counter = Arc::new(std::sync::Mutex::new(0_u8));
        let server_counter = Arc::clone(&counter);
        let server = TestServer::start(move |connection| {
            let mut number = server_counter.lock().expect("counter");
            connection.read_request();
            if *number == 0 {
                connection
                    .write_all(&canned_redirect(status, "/landed"))
                    .expect("redirect");
            } else {
                connection
                    .write_all(&canned_ok(&[], b"ok"))
                    .expect("landing");
            }
            *number += 1;
        });
        Agent::new()
            .request(method, server.url("/start"))
            .body(b"field=1")
            .send()
            .expect("redirect");
        let requests = server.requests();
        assert_eq!(requests[0].method, initial_method);
        assert_eq!(requests[1].method, expected_method);
        assert_eq!(requests[1].body, expected_body);
        server.assert_clean();
    }

    let counter = Arc::new(std::sync::Mutex::new(0_u8));
    let server_counter = Arc::clone(&counter);
    let server = TestServer::start(move |connection| {
        let mut number = server_counter.lock().expect("counter");
        connection.read_request();
        if *number < 2 {
            connection
                .write_all(&canned_redirect(302, "/next"))
                .expect("redirect");
        } else {
            connection
                .write_all(&canned_ok(&[], b"landed"))
                .expect("landing");
        }
        *number += 1;
    });
    let asked = server.url("/start#fragment");
    let response = Agent::new()
        .request(Method::GET, asked)
        .send()
        .expect("chain");
    assert_eq!(response.status(), 200);
    assert_eq!(response.final_url(), &server.url("/next#fragment"));
    assert_eq!(server.requests().len(), 3);
    server.assert_clean();

    let server = TestServer::start(|connection| {
        connection.read_request();
        connection
            .write_all(&canned_redirect(302, "/loop"))
            .expect("loop");
    });
    assert!(matches!(
        AgentBuilder::new()
            .max_redirects(2)
            .build()
            .request(Method::GET, server.url("/loop"))
            .send(),
        Err(NetError::Limit(LimitExceeded::Redirect))
    ));
    assert_eq!(server.requests().len(), 3);
    server.assert_clean();

    let landing = TestServer::start(|connection| {
        let request = connection.read_request();
        assert!(request.header("authorization").is_none());
        connection
            .write_all(&canned_ok(&[], b"landed"))
            .expect("landing");
    });
    let first = TestServer::start({
        let location = format!("http://{}/landed", landing.local_addr());
        move |connection| {
            connection.read_request();
            connection
                .write_all(&canned_redirect(302, &location))
                .expect("cross-origin redirect");
        }
    });
    Agent::new()
        .request(Method::GET, first.url("/start"))
        .header("Authorization", "Bearer secret")
        .expect("authorization")
        .send()
        .expect("cross-origin redirect");
    first.assert_clean();
    landing.assert_clean();
}

#[test]
fn proxy_tls_environment_and_debug_boundaries_stay_explicit() {
    for invalid in [
        "",
        "not a uri",
        "socks5://127.0.0.1:1080",
        "ftp://127.0.0.1:8080",
        "http://",
    ] {
        assert!(matches!(
            AgentBuilder::new().proxy(invalid),
            Err(NetError::Protocol(ProtocolError::InvalidProxy))
        ));
    }

    let proxy = TestServer::start(|connection| {
        let request = connection.read_request();
        assert_eq!(request.target, "/via");
        assert_eq!(request.header("host"), Some("origin.test"));
        assert_eq!(
            request.header("proxy-authorization"),
            Some("Basic dXNlcjpzZWNyZXQ=")
        );
        connection
            .write_all(&canned_ok(&[], b"via"))
            .expect("proxy response");
    });
    let proxy_uri = format!("http://user:secret@{}", proxy.local_addr());
    let response = AgentBuilder::new()
        .proxy(&proxy_uri)
        .expect("proxy")
        .build()
        .request(
            Method::GET,
            url::Url::parse("http://origin.test/via").expect("origin"),
        )
        .send()
        .expect("proxy request");
    assert_eq!(response.into_body().bytes(8).expect("body"), b"via");
    proxy.assert_clean();

    let builder = AgentBuilder::new()
        .proxy("http://user:secret@localhost:8080")
        .expect("proxy");
    let debug = format!("{builder:?}");
    assert!(!debug.contains("secret"));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("TLS listener");
    let addr = listener.local_addr().expect("address");
    let worker = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            use std::io::Write as _;
            let _ = stream.write_all(b"NOT-TLS");
        }
    });
    assert!(matches!(
        Agent::new()
            .request(
                Method::GET,
                url::Url::parse(&format!("https://{addr}/")).expect("https"),
            )
            .send(),
        Err(NetError::Transport(TransportError::Tls(reason))) if !reason.is_empty()
    ));
    worker.join().expect("TLS worker");

    if std::env::var(PROXY_PROBE_FLAG).ok().as_deref() == Some("1") {
        let server = TestServer::start(|connection| {
            connection.read_request();
            connection
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .expect("direct response");
        });
        Agent::new()
            .request(Method::GET, server.url("/direct"))
            .send()
            .expect("environment proxy must be ignored");
        server.assert_clean();
    } else {
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").expect("proxy bind");
        let proxy_uri = format!("http://{}", proxy.local_addr().expect("proxy address"));
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "proxy_tls_environment_and_debug_boundaries_stay_explicit",
            ])
            .env(PROXY_PROBE_FLAG, "1")
            .env("HTTP_PROXY", &proxy_uri)
            .env("http_proxy", &proxy_uri)
            .env("HTTPS_PROXY", &proxy_uri)
            .env("https_proxy", &proxy_uri)
            .env("ALL_PROXY", &proxy_uri)
            .env("all_proxy", &proxy_uri)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .expect("proxy probe");
        assert!(status.success());
    }
}
