//! `send()` end-to-end over loopback: ticket 11's acceptance criteria.
//!
//! Every test goes through the public API only — no backend type escapes
//! `net`, and the recording server sees exactly what hit the wire.

mod common;

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::TestServer;
use net::{Agent, AgentBuilder, LimitExceeded, Method, NetError, TimeoutKind, TransportError};

/// How long the client side waits on a server-side observation before
/// declaring the contract broken.
const OBSERVE_TIMEOUT: Duration = Duration::from_secs(5);

/// 64 KiB of deterministic patterned bytes: big enough to force several
/// chunks through any sane buffer size, varied enough to catch reordering.
fn patterned_body(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| u8::try_from(i % 251).expect("251 fits in u8"))
        .collect()
}

/// Canned `200 OK` response with headers and a body.
fn canned_ok(headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
    for (name, value) in headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    out.extend_from_slice(body);
    out
}

/// Poll `flag` until true, failing with `what` when it never happens.
fn await_flag(flag: &AtomicBool, what: &str) {
    let deadline = Instant::now() + OBSERVE_TIMEOUT;
    while !flag.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "never observed: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn get_round_trips_status_headers_and_streamed_body() {
    let body = b"hello from loopback";
    let server = TestServer::start(move |conn| {
        conn.read_request();
        let canned = canned_ok(
            &[("Content-Type", "text/plain"), ("X-Mixed-Case", "Value")],
            body,
        );
        conn.write_all(&canned).expect("canned write");
    });

    let response = Agent::new()
        .request(Method::GET, server.url("/a/b?c=d"))
        .send()
        .expect("loopback GET succeeds");

    assert_eq!(response.status(), 200);
    // Lookup is ASCII-case-insensitive regardless of stored case.
    assert_eq!(
        response.headers().get("content-type"),
        Some(&b"text/plain"[..])
    );
    assert_eq!(
        response.headers().get("CONTENT-TYPE"),
        Some(&b"text/plain"[..])
    );
    assert_eq!(response.headers().get("x-mixed-case"), Some(&b"Value"[..]));
    // The final URL is what was asked for (no redirects happened).
    assert_eq!(response.final_url(), &server.url("/a/b?c=d"));
    // Buffered read through the streaming body.
    assert_eq!(response.into_body().bytes(1024).expect("body reads"), body);

    let recorded = &server.requests()[0];
    assert_eq!(recorded.method, "GET");
    assert_eq!(recorded.target, "/a/b?c=d");
    assert_eq!(recorded.version, "HTTP/1.1");
    assert_eq!(
        recorded.header("host").map(str::to_owned),
        Some(server.local_addr().to_string())
    );
    server.assert_clean();
}

#[test]
fn every_http_status_arrives_as_data() {
    // 3xx statuses are excluded: the v1 backend follows redirects itself,
    // and a canned 3xx without (or with looping) Location is ticket 12's
    // redirect-cap territory. 4xx/5xx prove the statuses-as-data contract.
    for status in [201u16, 404, 500] {
        let server = TestServer::start(move |conn| {
            conn.read_request();
            let canned = format!("HTTP/1.1 {status} Whatever\r\nContent-Length: 2\r\n\r\nno");
            conn.write_all(canned.as_bytes()).expect("canned write");
        });
        let response = Agent::new()
            .request(Method::GET, server.url("/"))
            .send()
            .expect("status is data, not error");
        assert_eq!(response.status(), status);
        assert_eq!(response.into_body().bytes(16).expect("body"), b"no");
        server.assert_clean();
    }
}

#[test]
fn transport_failures_surface_as_neterror_transport() {
    // A port with no listener: bind, learn the port, drop the listener.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").expect("bind works");
    let addr = dead.local_addr().expect("addr known");
    drop(dead);

    let url = url::Url::parse(&format!("http://{addr}/")).expect("absolute");
    let result = Agent::new().request(Method::GET, url).send();

    match result {
        Err(NetError::Transport(_)) => {}
        other => panic!("expected NetError::Transport, got {other:?}"),
    }
}

#[test]
fn global_timeout_fires_when_the_head_never_arrives() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        // Head deliberately withheld past the client's budget: send() must
        // time out, not hang on the socket.
        std::thread::sleep(Duration::from_millis(600));
    });

    let result = AgentBuilder::new()
        .timeout_global(Duration::from_millis(120))
        .build()
        .request(Method::GET, server.url("/stall"))
        .send();

    match result {
        Err(NetError::Transport(TransportError::Timeout(kind))) => {
            assert_eq!(kind, TimeoutKind::Global);
        }
        other => panic!("expected Transport(Timeout(Global)), got {other:?}"),
    }
    server.assert_clean();
}

#[test]
fn chunked_transfer_decodes_to_a_clean_body_stream() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        // Two chunks + terminating zero chunk; read_chunk/bytes must see
        // only dechunked payload bytes (fetch streaming rides this under
        // the v1 backend).
        conn.write_all(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
              5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        )
        .expect("chunked write");
    });

    let response = Agent::new()
        .request(Method::GET, server.url("/chunked"))
        .send()
        .expect("chunked dial");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.into_body().bytes(64).expect("body reads"),
        &b"hello world"[..]
    );
    server.assert_clean();
}

#[test]
fn body_streams_incrementally_not_buffer_until_end() {
    let owned = patterned_body(100 * 1024);
    let expected = owned.clone();
    let expected_len = expected.len();
    let first_piece = owned[..1024].to_vec();
    let rest = owned[1024..].to_vec();

    // The server writes only the first kilobyte, then blocks until the
    // CLIENT proves a chunk was already delivered — only then does it
    // send the remaining bytes. A read_chunk that buffered until end of
    // body would deadlock here and the server-side timeout would fail
    // the test through assert_clean.
    let first_chunk_delivered = Arc::new(AtomicBool::new(false));
    let server_flag = Arc::clone(&first_chunk_delivered);
    let server = TestServer::start(move |conn| {
        conn.read_request();
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {expected_len}\r\n\r\n");
        conn.write_all(head.as_bytes()).expect("head write");
        conn.write_all(&first_piece).expect("first piece");

        let deadline = Instant::now() + OBSERVE_TIMEOUT;
        while !server_flag.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "read_chunk never delivered before body completion: not incremental"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        conn.write_all(&rest).expect("rest write");
    });

    let mut body = Agent::new()
        .request(Method::GET, server.url("/big"))
        .send()
        .expect("GET succeeds")
        .into_body();

    let mut collected = Vec::new();
    let mut chunk_count = 0;
    while let Some(chunk) = body.read_chunk().expect("chunk reads") {
        collected.extend_from_slice(&chunk);
        chunk_count += 1;
        first_chunk_delivered.store(true, Ordering::Release);
    }
    assert_eq!(collected, expected);
    assert!(chunk_count > 1, "streaming must yield multiple chunks");
    server.assert_clean();
}

#[test]
fn dropping_response_closes_the_connection() {
    // Observed on the server thread, asserted here: the client drops
    // mid-body and the server must see the socket close within the
    // deadline — cancellation IS drop, nothing pools a mid-body
    // connection.
    let peer_closed = Arc::new(AtomicBool::new(false));
    let server_flag = Arc::clone(&peer_closed);
    let total = 64 * 1024;
    let server = TestServer::start(move |conn| {
        conn.read_request();
        // Declare far more than we send so the client must drop mid-body.
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\r\n");
        conn.write_all(head.as_bytes()).expect("head write");
        conn.write_all(&vec![0xAA; 1024]).expect("partial body");

        server_flag.store(conn.await_peer_close(), Ordering::Release);
    });

    let agent = Agent::new();
    {
        let mut body = agent
            .request(Method::GET, server.url("/cancel"))
            .send()
            .expect("GET succeeds")
            .into_body();
        let _one_chunk = body.read_chunk().expect("first chunk reads");
        // Scope ends: Response and Body drop with unread bytes pending.
    }

    await_flag(&peer_closed, "server observing the client closing on drop");
    server.assert_clean();
}

#[test]
fn request_headers_reach_the_wire_in_insertion_order() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("canned write");
    });

    let builder = Agent::new()
        .request(Method::GET, server.url("/h"))
        .header("X-Custom-Thing", "alpha")
        .expect("valid header")
        .header("Accept", "text/html")
        .expect("valid header")
        .header("x-lowercase-name", "gamma")
        .expect("valid header");
    builder.send().expect("send works");

    let recorded = &server.requests()[0];
    let names: Vec<&str> = recorded.headers.iter().map(|(n, _)| n.as_str()).collect();
    // Insertion order survives end to end.
    let pos = |needle: &str| names.iter().position(|n| n.eq_ignore_ascii_case(needle));
    let (custom, accept, lowercase) = (
        pos("x-custom-thing"),
        pos("accept"),
        pos("x-lowercase-name"),
    );
    assert!(custom.unwrap() < accept.unwrap() && accept.unwrap() < lowercase.unwrap());

    // v1 fidelity caveat (decision 02): the backend normalizes wire names
    // to lowercase in both directions; values keep their exact bytes. The
    // stealth swap upgrades name fidelity without signature changes.
    assert_eq!(names.get(custom.unwrap()), Some(&"x-custom-thing"));
    assert_eq!(names.get(lowercase.unwrap()), Some(&"x-lowercase-name"));
    assert_eq!(recorded.header("x-custom-thing"), Some("alpha"));
    server.assert_clean();
}

#[test]
fn builder_user_agent_rides_the_wire() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("canned write");
    });

    AgentBuilder::new()
        .user_agent("tinybrowser-test/0.1")
        .build()
        .request(Method::GET, server.url("/ua"))
        .send()
        .expect("send works");

    let recorded = &server.requests()[0];
    // Exactly one UA on the wire: net owns the layering (agent config
    // supplies it only when the request does not set one) — never stacked
    // duplicates. The override direction is pinned by
    // `request_level_user_agent_overrides_the_builder`.
    let ua_count = recorded
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
        .count();
    assert_eq!(ua_count, 1, "user-agent must appear exactly once");
    assert_eq!(recorded.header("user-agent"), Some("tinybrowser-test/0.1"));
    server.assert_clean();
}

#[test]
fn request_level_user_agent_overrides_the_builder() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("canned write");
    });

    AgentBuilder::new()
        .user_agent("builder/default")
        .build()
        .request(Method::GET, server.url("/ua"))
        .header("User-Agent", "request/wins")
        .expect("valid header")
        .send()
        .expect("send works");

    let requests = server.requests();
    let uas: Vec<&str> = requests[0]
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
        .map(|(_, value)| value.as_str())
        .collect();
    // Explicit beats configured, and exactly one header survives — the
    // layering is decided in send(), not inherited from the backend.
    assert_eq!(uas, ["request/wins"]);
    server.assert_clean();
}

#[test]
fn default_agent_sends_no_backend_default_headers() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("canned write");
    });

    Agent::new()
        .request(Method::GET, server.url("/bare"))
        .send()
        .expect("send works");

    let recorded = &server.requests()[0];
    // Decision 02: net sends only headers we build ourselves. The
    // backend's own defaults (its version string, Accept, Accept-Encoding)
    // are suppressed at construction — wire behavior we don't own would
    // contradict the seam.
    for banned in ["user-agent", "accept", "accept-encoding"] {
        assert!(
            recorded.header(banned).is_none(),
            "backend-injected {banned} leaked onto the wire"
        );
    }
    server.assert_clean();
}

#[test]
fn post_sends_its_body_with_content_length() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("canned write");
    });

    Agent::new()
        .request(Method::POST, server.url("/submit"))
        .body(b"name=value".as_slice())
        .send()
        .expect("POST succeeds");

    let recorded = &server.requests()[0];
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.body, b"name=value");
    assert_eq!(recorded.header("content-length"), Some("10"));
    server.assert_clean();
}

#[test]
fn invalid_header_values_are_rejected_at_the_boundary() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("canned write");
    });

    // RFC 9110 §5.5 field-value grammar: control characters other than
    // HTAB are forbidden — this is the injection guard.
    let result = Agent::new()
        .request(Method::GET, server.url("/evil"))
        .header("X-Bad", "line one\r\nX-Forged: yes")
        .expect_err("CRLF injection rejected");
    assert!(matches!(result, net::HeaderError::InvalidValue(_)));

    // Nothing reached the wire: rejection happened before any dial.
    assert!(
        server.requests().is_empty(),
        "a rejected request must never dial — server saw {:?}",
        server.requests()
    );
    server.assert_clean();
}

#[test]
fn fragments_are_stripped_before_the_network() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("canned write");
    });

    // Navigation to anchored URLs is the common case; the fragment must
    // never reach the request line, and the final URL must not carry one.
    let anchored = {
        let mut url = server.url("/page#section-2");
        url.set_fragment(None);
        url
    };
    let response = Agent::new()
        .request(Method::GET, server.url("/page#section-2"))
        .send()
        .expect("send works");

    assert_eq!(server.requests()[0].target, "/page");
    assert_eq!(response.final_url(), &anchored);
    server.assert_clean();
}

#[test]
fn redirect_chain_updates_final_url_and_records_every_hop() {
    let server = TestServer::start(|conn| {
        let req = conn.read_request();
        if req.target == "/a" {
            conn.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /b\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("redirect write");
        } else {
            conn.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfinal",
            )
            .expect("landing write");
        }
    });

    let response = Agent::new()
        .request(Method::GET, server.url("/a"))
        .send()
        .expect("redirects resolve");

    assert_eq!(response.status(), 200);
    // final_url is the post-redirect location; the original URL is not
    // silently substituted (Firefox's URI-vs-originalURI split).
    assert_eq!(response.final_url(), &server.url("/b"));
    assert_eq!(
        response.into_body().bytes(16).expect("body reads"),
        b"final"
    );
    let requests = server.requests();
    let targets: Vec<&str> = requests.iter().map(|req| req.target.as_str()).collect();
    assert_eq!(targets, ["/a", "/b"], "every hop hits the wire in order");
    server.assert_clean();
}

#[test]
fn redirect_cap_fires_as_limit_redirect_and_bounds_the_loop() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(
            b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .expect("loop write");
    });

    let cap = 2;
    let result = AgentBuilder::new()
        .max_redirects(cap)
        .build()
        .request(Method::GET, server.url("/loop"))
        .send();

    match result {
        Err(NetError::Limit(LimitExceeded::Redirect)) => {}
        other => panic!("expected Limit(Redirect), got {other:?}"),
    }
    // The cap must actually bound the loop (no exact-hop pinning: how the
    // backend counts against the cap is its own semantics).
    let hops = server.requests().len();
    assert!(hops >= 2, "at least one redirect followed before the cap");
    // Lossless widening (u32 → usize); `as` is the honest form, there is
    // no From<u32> for usize on this toolchain.
    assert!(hops <= cap as usize + 1, "cap bound the loop, saw {hops}");
    for recorded in server.requests() {
        assert_eq!(recorded.target, "/loop");
    }
    server.assert_clean();
}

#[test]
fn method_case_is_fetch_accurate_at_the_type_and_on_the_wire() {
    // Type level: parse keeps exactly the casing WHATWG fetch would put
    // on the wire — #methods' normalize list is exhaustive, `patch` is
    // deliberately outside it, extension tokens stay verbatim.
    for (parsed, want) in [
        ("get", "GET"),
        ("head", "HEAD"),
        ("patch", "patch"),
        ("PATCH", "PATCH"),
        ("propfind", "propfind"),
        ("eGg", "eGg"),
    ] {
        let method = Method::parse(parsed).expect("valid token");
        assert_eq!(method.as_str(), want, "parse({parsed:?}) wire token");
    }

    // Wire level: those same tokens reach the request line. v1 enables
    // ureq's `allow_non_standard_methods` so fetch-accurate casing is not
    // a stealth-only property.
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("canned write");
    });

    let agent = Agent::new();
    let expected = ["GET", "HEAD", "PATCH", "patch", "propfind", "eGg"];
    for token in expected {
        let sent = Method::parse(token).expect("valid token");
        agent
            .request(sent, server.url("/m"))
            .send()
            .expect("fetch-accurate methods dial");
    }
    let recorded = server.requests();
    let methods: Vec<&str> = recorded.iter().map(|req| req.method.as_str()).collect();
    assert_eq!(methods, expected);
    server.assert_clean();
}

#[test]
fn buffered_body_enforces_the_caller_size_cap() {
    let payload = vec![b'x'; 32];
    let server = TestServer::start(move |conn| {
        conn.read_request();
        conn.write_all(&canned_ok(&[], &payload))
            .expect("canned write");
    });

    let body = Agent::new()
        .request(Method::GET, server.url("/sized"))
        .send()
        .expect("dial")
        .into_body();
    match body.bytes(16) {
        Err(NetError::Limit(LimitExceeded::Size(cap))) => assert_eq!(cap, 16),
        other => panic!("expected Limit(Size(16)), got {other:?}"),
    }
    server.assert_clean();

    // Exact fit is allowed: the cap is exclusive (`>`), not `>=`.
    let payload = vec![b'y'; 16];
    let server = TestServer::start(move |conn| {
        conn.read_request();
        conn.write_all(&canned_ok(&[], &payload))
            .expect("canned write");
    });
    let got = Agent::new()
        .request(Method::GET, server.url("/exact"))
        .send()
        .expect("dial")
        .into_body()
        .bytes(16)
        .expect("exact fit");
    assert_eq!(got, vec![b'y'; 16]);
    server.assert_clean();
}

#[test]
fn post_307_replays_method_and_body() {
    // fetch #http-redirect-fetch: 307 keeps POST + body. ureq used to
    // abort this as RedirectFailed; net owns the follow loop so it must
    // not.
    let server = TestServer::start(|conn| {
        let req = conn.read_request();
        if req.target == "/a" {
            conn.write_all(
                b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /b\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("redirect write");
        } else {
            conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .expect("landing write");
        }
    });

    let response = Agent::new()
        .request(Method::POST, server.url("/a"))
        .body(b"payload".as_slice())
        .send()
        .expect("307 POST follows");

    assert_eq!(response.status(), 200);
    assert_eq!(response.final_url(), &server.url("/b"));
    assert_eq!(response.into_body().bytes(8).expect("body"), b"ok");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].body, b"payload");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].target, "/b");
    assert_eq!(requests[1].body, b"payload");
    server.assert_clean();
}

#[test]
fn post_302_becomes_get_without_the_body() {
    let server = TestServer::start(|conn| {
        let req = conn.read_request();
        if req.target == "/a" {
            conn.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /b\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("redirect write");
        } else {
            conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("landing write");
        }
    });

    Agent::new()
        .request(Method::POST, server.url("/a"))
        .body(b"payload".as_slice())
        .send()
        .expect("302 POST follows as GET");

    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].body, b"payload");
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].target, "/b");
    assert!(requests[1].body.is_empty());
    assert!(requests[1].header("content-length").is_none());
    server.assert_clean();
}

#[test]
fn cookie_header_survives_a_same_origin_redirect() {
    let server = TestServer::start(|conn| {
        let req = conn.read_request();
        if req.target == "/a" {
            conn.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /b\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("redirect write");
        } else {
            conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("landing write");
        }
    });

    Agent::new()
        .request(Method::GET, server.url("/a"))
        .header("Cookie", "a=1")
        .expect("valid header")
        .send()
        .expect("redirects");

    let requests = server.requests();
    assert_eq!(requests[0].header("cookie"), Some("a=1"));
    assert_eq!(requests[1].header("cookie"), Some("a=1"));
    server.assert_clean();
}

#[test]
fn agent_ignores_env_http_proxy() {
    // Prove `.proxy(None)` without mutating this process: a child gets
    // HTTP_PROXY/ALL_PROXY via Command::env (safe) and must still dial
    // loopback, not CONNECT to the unaccepted listener we hold open.
    const FLAG: &str = "NET_CRATE_PROXY_PROBE";
    if std::env::var(FLAG).ok().as_deref() != Some("1") {
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").expect("proxy bind");
        let proxy_uri = format!("http://{}", proxy.local_addr().expect("proxy addr"));
        let status = Command::new(std::env::current_exe().expect("test exe"))
            .args(["--exact", "agent_ignores_env_http_proxy"])
            .env(FLAG, "1")
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
            .expect("re-exec proxy probe");
        assert!(status.success(), "child proxy probe failed: {status}");
        return;
    }

    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("canned write");
    });

    let response = AgentBuilder::new()
        .timeout_global(Duration::from_millis(500))
        .build()
        .request(Method::GET, server.url("/direct"))
        .send()
        .expect("loopback must not go through HTTP_PROXY");
    assert_eq!(response.status(), 200);
    assert_eq!(server.requests()[0].target, "/direct");
    server.assert_clean();
}
