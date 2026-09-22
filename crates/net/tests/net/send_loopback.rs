use super::common::{TestServer, canned_ok, canned_redirect, scripted};
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use net::{
    Agent, AgentOptions, LimitExceeded, Method, NetError, ProtocolError, Request, TimeoutKind,
    TransportError,
};

const OBSERVE_TIMEOUT: Duration = Duration::from_secs(5);
const PROXY_PROBE_FLAG: &str = "NET_CRATE_PROXY_PROBE";

fn default_agent() -> Agent {
    Agent::new(AgentOptions::default()).expect("default options are valid")
}

#[tokio::test]
async fn http_transcripts_cover_status_headers_framing_limits_and_failures() {
    for status in [201_u16, 302, 404, 500] {
        let server =
            scripted([
                format!("HTTP/1.1 {status} Whatever\r\nContent-Length: 2\r\n\r\nno").into_bytes(),
            ]);
        let response = default_agent()
            .send(Request::new(Method::GET, server.url("/")))
            .await
            .expect("status is response data");
        assert_eq!(response.status(), status);
        assert_eq!(response.into_body().bytes(16).await.expect("body"), b"no");
        server.assert_clean();
    }

    let server = scripted([canned_ok(
        &[("Content-Type", "text/plain"), ("X-Mixed-Case", "Value")],
        b"hello",
    )]);
    let requested = server.url("/a/b?c=d");
    let response = default_agent()
        .send(Request::new(Method::GET, requested.clone()))
        .await
        .expect("GET");
    let request = &server.requests()[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.target, "/a/b?c=d");
    assert_eq!(request.version, "HTTP/1.1");
    assert_eq!(response.status(), 200);
    assert_eq!(response.final_url(), &requested);
    assert_eq!(
        response.headers().get("CONTENT-TYPE"),
        Some(&b"text/plain"[..])
    );
    assert_eq!(
        response.into_body().bytes(16).await.expect("body"),
        b"hello"
    );
    server.assert_clean();

    let server = scripted([b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n".to_vec()]);
    assert_eq!(
        default_agent()
            .send(Request::new(Method::GET, server.url("/chunked")))
            .await
            .expect("chunked")
            .into_body()
            .bytes(64)
            .await
            .expect("body"),
        b"hello world"
    );
    server.assert_clean();

    let payload = vec![b'x'; 32];
    let server = scripted([canned_ok(&[], &payload)]);
    assert!(matches!(
        default_agent()
            .send(Request::new(Method::GET, server.url("/limit")))
            .await
            .expect("response")
            .into_body()
            .bytes(16)
            .await,
        Err(NetError::Limit(LimitExceeded::Size(16)))
    ));
    server.assert_clean();

    let dead = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let dead_url = format!("http://{}/", dead.local_addr().expect("address"));
    drop(dead);
    assert!(matches!(
        default_agent()
            .send(Request::new(
                Method::GET,
                url::Url::parse(&dead_url).expect("url")
            ))
            .await,
        Err(NetError::Transport(_))
    ));

    let server = TestServer::start(|connection| {
        connection.read_request();
        std::thread::sleep(Duration::from_millis(300));
    });
    assert!(matches!(
        Agent::new(AgentOptions {
            timeout_global: Some(Duration::from_millis(60)),
            ..AgentOptions::default()
        })
        .expect("timeout options")
        .send(Request::new(Method::GET, server.url("/stall")))
        .await,
        Err(NetError::Transport(TransportError::Timeout(
            TimeoutKind::Global
        )))
    ));
    server.assert_clean();
}

#[tokio::test]
async fn timeout_global_covers_every_redirect_hop() {
    let hop_delay = Duration::from_millis(80);
    let global = Duration::from_millis(150);
    let hops = Arc::new(std::sync::Mutex::new(0_u8));
    let server_hops = Arc::clone(&hops);
    let server = TestServer::start(move |connection| {
        connection.read_request();
        std::thread::sleep(hop_delay);
        let mut number = server_hops.lock().expect("hop counter");
        *number = number.saturating_add(1);
        let payload = if *number < 3 {
            canned_redirect(302, "/next")
        } else {
            canned_ok(&[], b"landed")
        };
        let _ = connection.write_all(&payload);
    });
    let started = Instant::now();
    let result = Agent::new(AgentOptions {
        timeout_global: Some(global),
        ..AgentOptions::default()
    })
    .expect("timeout options")
    .send(Request::new(Method::GET, server.url("/start")))
    .await;
    let elapsed = started.elapsed();
    assert!(
        matches!(
            result,
            Err(NetError::Transport(TransportError::Timeout(
                TimeoutKind::Global
            )))
        ),
        "expected global timeout, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_millis(400),
        "absolute deadline waited {elapsed:?}"
    );
    assert!(hop_delay < global);
    assert!(hop_delay.saturating_mul(3) > global);
    server.assert_clean();
}

#[tokio::test]
async fn response_bodies_stream_and_drop_cancels_the_socket() {
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
    let mut body = default_agent()
        .send(Request::new(Method::GET, server.url("/stream")))
        .await
        .expect("stream")
        .into_body();
    let mut collected = Vec::new();
    let mut chunks = 0;
    while let Some(chunk) = body.read_chunk().await.expect("chunk") {
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
        let mut body = default_agent()
            .send(Request::new(Method::GET, server.url("/cancel")))
            .await
            .expect("cancel")
            .into_body();
        let _ = body.read_chunk().await.expect("partial chunk");
    }
    let deadline = Instant::now() + OBSERVE_TIMEOUT;
    while !peer_closed.load(Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "server never observed the client"
        );
        tokio::task::yield_now().await;
    }
    server.assert_clean();
}

#[tokio::test]
async fn request_shaping_custom_method_fragments_and_rejections_are_wire_visible() {
    let server = scripted([b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec()]);
    default_agent()
        .send(Request::new(
            Method::parse("propfind").expect("custom method"),
            server.url("/m"),
        ))
        .await
        .expect("custom method request");
    assert_eq!(server.requests()[0].method, "propfind");
    server.assert_clean();

    let server = scripted([b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec()]);
    let agent = Agent::new(AgentOptions {
        user_agent: Some("builder/default".to_owned()),
        ..AgentOptions::default()
    })
    .expect("agent options");
    let mut request = Request::new(Method::POST, server.url("/shape#fragment"));
    request.headers.insert("X-Custom", "alpha").expect("header");
    request
        .headers
        .insert("User-Agent", "request/wins")
        .expect("header");
    request.body = Some(b"name=value".to_vec());
    agent.send(request).await.expect("shaped request");
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
    let mut request = Request::new(Method::GET, server.url("/invalid"));
    let result = request.headers.insert("X-Bad", "line\r\nInjected: yes");
    assert!(matches!(result, Err(net::HeaderError::InvalidValue(_))));
    assert!(server.requests().is_empty());
    server.assert_clean();
}

/// Our redirect cap and its typed error. Redirect *policy* (method rewriting,
/// header stripping, fragment handling) is WPT's `fetch/api/redirect/` suite.
#[tokio::test]
async fn max_redirects_cap_returns_limit_exceeded() {
    let empty = scripted([canned_redirect(302, ""), canned_redirect(302, "")]);
    assert!(matches!(
        Agent::new(AgentOptions {
            max_redirects: 1,
            ..AgentOptions::default()
        })
        .expect("cap options")
        .send(Request::new(Method::GET, empty.url("/empty")))
        .await,
        Err(NetError::Limit(LimitExceeded::Redirect))
    ));
    assert_eq!(empty.requests().len(), 2);
    empty.assert_clean();

    let server = scripted(std::iter::repeat_n(canned_redirect(302, "/loop"), 3));
    assert!(matches!(
        Agent::new(AgentOptions {
            max_redirects: 2,
            ..AgentOptions::default()
        })
        .expect("cap options")
        .send(Request::new(Method::GET, server.url("/loop")))
        .await,
        Err(NetError::Limit(LimitExceeded::Redirect))
    ));
    assert_eq!(server.requests().len(), 3);
    server.assert_clean();
}

#[tokio::test]
async fn connect_proxy_routes_https_and_reports_denials() {
    let proxy = TestServer::start(|connection| {
        let request = connection.read_request();
        assert_eq!(request.method, "CONNECT");
        assert_eq!(request.target, "origin.test:443");
        assert_eq!(
            request.header("proxy-authorization"),
            Some("Basic dXNlcjpzZWNyZXQ=")
        );
        connection
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\nNOT-TLS")
            .expect("connect 200");
        std::thread::sleep(Duration::from_millis(300));
    });
    let proxy_uri = format!("http://user:secret@{}", proxy.local_addr());
    let err = Agent::new(AgentOptions {
        proxy: Some(proxy_uri),
        ..AgentOptions::default()
    })
    .expect("proxy")
    .send(Request::new(
        Method::GET,
        url::Url::parse("https://origin.test/").expect("https"),
    ))
    .await
    .expect_err("proxy request");
    assert!(
        matches!(
            err,
            NetError::Transport(TransportError::Connect(_) | TransportError::Tls(_))
        ),
        "unexpected proxy error: {err:?}"
    );
    proxy.assert_clean();

    let denied = TestServer::start(|connection| {
        connection.read_request();
        connection
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            .expect("connect 403");
    });
    assert!(matches!(
        Agent::new(AgentOptions {
            proxy: Some(format!("http://{}", denied.local_addr())),
            ..AgentOptions::default()
        })
        .expect("proxy")
        .send(Request::new(
            Method::GET,
            url::Url::parse("https://origin.test/").expect("https"),
        ))
        .await,
        Err(NetError::Transport(TransportError::Connect(_)))
    ));
    denied.assert_clean();
}

#[tokio::test]
async fn proxy_tls_environment_and_debug_boundaries_stay_explicit() {
    for invalid in [
        "",
        "not a uri",
        "socks5://127.0.0.1:1080",
        "ftp://127.0.0.1:8080",
        "http://",
    ] {
        assert!(matches!(
            Agent::new(AgentOptions {
                proxy: Some(invalid.to_owned()),
                ..AgentOptions::default()
            }),
            Err(NetError::Protocol(ProtocolError::InvalidProxy))
        ));
    }

    let options = AgentOptions {
        proxy: Some("http://user:secret@localhost:8080".to_owned()),
        ..AgentOptions::default()
    };
    let debug = format!("{options:?}");
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
        default_agent()
            .send(Request::new(
                Method::GET,
                url::Url::parse(&format!("https://{addr}/")).expect("https"),
            ))
            .await,
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
        default_agent()
            .send(Request::new(Method::GET, server.url("/direct")))
            .await
            .expect("environment proxy must be ignored");
        server.assert_clean();
    } else {
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").expect("proxy bind");
        let proxy_uri = format!("http://{}", proxy.local_addr().expect("proxy address"));
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "send_loopback::proxy_tls_environment_and_debug_boundaries_stay_explicit",
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

#[tokio::test]
async fn ipv6_loopback_send_and_connect_paths_are_exercised() {
    let server = TestServer::start_v6(|connection| {
        let request = connection.read_request();
        assert_eq!(request.target, "/v6");
        connection
            .write_all(&canned_ok(&[], b"v6"))
            .expect("v6 response");
    });
    let response = default_agent()
        .send(Request::new(Method::GET, server.url("/v6")))
        .await
        .expect("ipv6 send");
    assert_eq!(response.into_body().bytes(8).await.expect("body"), b"v6");
    server.assert_clean();

    let wss = std::net::TcpListener::bind("127.0.0.1:0").expect("wss listener");
    let wss_addr = wss.local_addr().expect("address");
    let worker = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = wss.accept() {
            use std::io::Write as _;
            let _ = stream.write_all(b"NOT-TLS");
        }
    });
    assert!(matches!(
        default_agent()
            .upgrade(Request::new(
                Method::GET,
                url::Url::parse(&format!("wss://{wss_addr}/")).expect("wss"),
            ))
            .await,
        Err(NetError::Transport(TransportError::Tls(_)) | NetError::Protocol(_))
    ));
    worker.join().expect("wss worker");
}

#[tokio::test]
async fn url_credentials_and_scheme_rejections_are_wire_visible() {
    let server = TestServer::start(|connection| {
        let request = connection.read_request();
        assert_eq!(request.header("authorization"), Some("Basic dXNlcjpwYXNz"));
        connection
            .write_all(&canned_ok(&[], b"ok"))
            .expect("auth response");
    });
    let mut url = server.url("/");
    url.set_username("user").expect("username");
    url.set_password(Some("pass")).expect("password");
    default_agent()
        .send(Request::new(Method::GET, url))
        .await
        .expect("basic");
    server.assert_clean();

    assert!(matches!(
        default_agent()
            .send(Request::new(
                Method::GET,
                url::Url::parse("ws://127.0.0.1/").expect("ws"),
            ))
            .await,
        Err(NetError::Protocol(ProtocolError::RejectedRequest))
    ));
}

#[tokio::test]
async fn resolve_maps_hit_miss_and_fail() {
    assert!(matches!(
        Agent::new(AgentOptions {
            resolve: vec!["not-a-spec".to_owned()],
            ..AgentOptions::default()
        }),
        Err(NetError::Protocol(ProtocolError::InvalidResolve))
    ));

    let server = scripted([canned_ok(&[], b"mapped")]);
    let port = server.local_addr().port();
    let mapped = url::Url::parse(&format!("http://web-platform.test:{port}/")).expect("mapped URL");
    let agent = Agent::new(AgentOptions {
        resolve: vec![
            "nonexistent.*.test=fail".to_owned(),
            "*.test=127.0.0.1".to_owned(),
        ],
        ..AgentOptions::default()
    })
    .expect("resolve specs");
    let response = agent
        .send(Request::new(Method::GET, mapped))
        .await
        .expect("mapped hit");
    assert_eq!(
        response.into_body().bytes(16).await.expect("body"),
        b"mapped"
    );
    assert_eq!(
        server.requests()[0]
            .header("Host")
            .map(|value| value.split(':').next()),
        Some(Some("web-platform.test"))
    );
    server.assert_clean();

    let miss_server = scripted([canned_ok(&[], b"direct")]);
    let miss = agent
        .send(Request::new(Method::GET, miss_server.url("/")))
        .await
        .expect("unmapped 127.0.0.1 uses libc");
    assert_eq!(miss.into_body().bytes(16).await.expect("body"), b"direct");
    miss_server.assert_clean();

    let failed = agent.send(Request::new(
        Method::GET,
        url::Url::parse("http://nonexistent.web-platform.test/").expect("fail URL"),
    ));
    assert!(matches!(
        failed.await,
        Err(NetError::Transport(TransportError::Dns(_)))
    ));
}

#[tokio::test]
async fn request_deadline_expires_with_a_typed_timeout() {
    let server = TestServer::start(|connection| {
        connection.read_request();
        // Never respond; the client's absolute deadline must fire.
        std::thread::sleep(Duration::from_millis(600));
    });
    let mut request = Request::new(Method::GET, server.url("/stall"));
    request.deadline = Some(Instant::now() + Duration::from_millis(80));
    let error = default_agent()
        .send(request)
        .await
        .expect_err("deadline must expire");
    assert!(
        matches!(error, NetError::Transport(TransportError::Timeout(_))),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn https_dials_offer_h2_before_http1_in_alpn() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let hello = read_client_hello(&mut stream);
        let protocols = client_hello_alpn(&hello);
        // Hold the connection so the request deadline is the visible failure.
        std::thread::sleep(Duration::from_millis(600));
        protocols
    });

    let url = url::Url::parse(&format!("https://{address}/")).expect("absolute url");
    let mut request = Request::new(Method::GET, url);
    request.deadline = Some(Instant::now() + Duration::from_millis(80));
    let error = default_agent()
        .send(request)
        .await
        .expect_err("the test listener never completes a TLS handshake");
    assert!(
        matches!(error, NetError::Transport(TransportError::Timeout(_))),
        "unexpected error: {error}"
    );
    assert_eq!(
        server.join().expect("server"),
        vec!["h2".to_owned(), "http/1.1".to_owned()]
    );
}

fn read_client_hello(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is settable");
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        if bytes.len() >= 5 {
            let body = usize::from(u16::from_be_bytes([bytes[3], bytes[4]]));
            if bytes.len() >= 5 + body {
                bytes.truncate(5 + body);
                return bytes;
            }
        }
        let read = stream.read(&mut chunk).expect("client hello is readable");
        assert_ne!(read, 0, "client closed before sending a full ClientHello");
        bytes.extend_from_slice(&chunk[..read]);
    }
}

fn client_hello_alpn(hello: &[u8]) -> Vec<String> {
    assert_eq!(hello.first(), Some(&0x16), "not a TLS handshake record");
    assert_eq!(hello.get(5), Some(&0x01), "not a ClientHello");
    let mut at = 9;
    at += 2 + 32;
    at += 1 + usize::from(hello[at]);
    let suites = usize::from(u16::from_be_bytes([hello[at], hello[at + 1]]));
    at += 2 + suites;
    at += 1 + usize::from(hello[at]);
    let extensions = usize::from(u16::from_be_bytes([hello[at], hello[at + 1]]));
    at += 2;
    let end = at + extensions;
    while at + 4 <= end {
        let kind = usize::from(u16::from_be_bytes([hello[at], hello[at + 1]]));
        let length = usize::from(u16::from_be_bytes([hello[at + 2], hello[at + 3]]));
        at += 4;
        if kind == 16 {
            let list = usize::from(u16::from_be_bytes([hello[at], hello[at + 1]]));
            at += 2;
            let mut protocols = Vec::new();
            let mut entry = at;
            while entry < at + list {
                let len = usize::from(hello[entry]);
                entry += 1;
                protocols.push(
                    String::from_utf8_lossy(&hello[entry..entry + len]).into_owned(),
                );
                entry += len;
            }
            return protocols;
        }
        at += length;
    }
    Vec::new()
}
