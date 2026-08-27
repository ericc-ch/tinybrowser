//! Cookie jar through the public API (ticket 13).

mod common;

use common::TestServer;
use net::{Agent, Context, Method};

fn canned_set_cookie(set_cookie: &str, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
    out.extend_from_slice(format!("Set-Cookie: {set_cookie}\r\n").as_bytes());
    out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    out.extend_from_slice(body);
    out
}

fn canned_ok(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
    out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    out.extend_from_slice(body);
    out
}

#[test]
fn loopback_set_cookie_is_replayed_on_the_next_request() {
    let hops = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let server_hops = std::sync::Arc::clone(&hops);
    let server = TestServer::start(move |conn| {
        conn.read_request();
        let n = {
            let mut g = server_hops.lock().expect("hops");
            let n = *g;
            *g += 1;
            n
        };
        if n == 0 {
            conn.write_all(&canned_set_cookie("sid=abc; Path=/", b"one"))
                .expect("set-cookie");
        } else {
            conn.write_all(&canned_ok(b"two")).expect("ok");
        }
    });

    let agent = Agent::new();
    agent
        .request(Method::GET, server.url("/"))
        .send()
        .expect("first");
    agent
        .request(Method::GET, server.url("/"))
        .send()
        .expect("second");

    let recorded = server.requests();
    assert!(recorded[0].header("cookie").is_none());
    assert_eq!(recorded[1].header("cookie"), Some("sid=abc"));
    server.assert_clean();
}

#[test]
fn path_attribute_scopes_which_requests_carry_the_cookie() {
    let hops = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let server_hops = std::sync::Arc::clone(&hops);
    let server = TestServer::start(move |conn| {
        conn.read_request();
        let n = {
            let mut g = server_hops.lock().expect("hops");
            let n = *g;
            *g += 1;
            n
        };
        if n == 0 {
            conn.write_all(&canned_set_cookie("p=1; Path=/docs", b"x"))
                .expect("set");
        } else {
            conn.write_all(&canned_ok(b"y")).expect("ok");
        }
    });
    let agent = Agent::new();
    agent
        .request(Method::GET, server.url("/docs/guide"))
        .send()
        .expect("set");
    agent
        .request(Method::GET, server.url("/docs/guide"))
        .send()
        .expect("match");
    agent
        .request(Method::GET, server.url("/other"))
        .send()
        .expect("miss");

    let recorded = server.requests();
    assert_eq!(recorded[1].header("cookie"), Some("p=1"));
    assert!(recorded[2].header("cookie").is_none());
    server.assert_clean();
}

#[test]
fn host_only_cookie_does_not_carry_a_rejected_domain() {
    // Domain=example.com does not domain-match 127.0.0.1 — ignored at store.
    let hops = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let server_hops = std::sync::Arc::clone(&hops);
    let server = TestServer::start(move |conn| {
        conn.read_request();
        let n = {
            let mut g = server_hops.lock().expect("hops");
            let n = *g;
            *g += 1;
            n
        };
        if n == 0 {
            conn.write_all(&canned_set_cookie("x=1; Domain=example.com; Path=/", b"x"))
                .expect("set");
        } else {
            conn.write_all(&canned_ok(b"y")).expect("ok");
        }
    });
    let agent = Agent::new();
    agent
        .request(Method::GET, server.url("/"))
        .send()
        .expect("set");
    agent
        .request(Method::GET, server.url("/"))
        .send()
        .expect("get");
    assert!(server.requests()[1].header("cookie").is_none());
    server.assert_clean();
}

#[test]
fn httponly_is_hidden_from_cookies_for_but_still_sent() {
    let hops = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let server_hops = std::sync::Arc::clone(&hops);
    let server = TestServer::start(move |conn| {
        conn.read_request();
        let n = {
            let mut g = server_hops.lock().expect("hops");
            let n = *g;
            *g += 1;
            n
        };
        if n == 0 {
            conn.write_all(&canned_set_cookie("sid=s; Path=/; HttpOnly", b"x"))
                .expect("set");
        } else {
            conn.write_all(&canned_ok(b"y")).expect("ok");
        }
    });
    let agent = Agent::new();
    let uri = server.url("/");
    agent.request(Method::GET, uri.clone()).send().expect("set");
    assert!(
        agent.cookies_for(&uri).is_empty(),
        "HttpOnly must not appear in document.cookie"
    );
    agent.request(Method::GET, uri).send().expect("replay");
    assert_eq!(server.requests()[1].header("cookie"), Some("sid=s"));
    server.assert_clean();
}

#[test]
fn secure_cookies_never_leave_on_cleartext_http() {
    let https = url::Url::parse("https://example.com/").expect("https");
    let http = url::Url::parse("http://example.com/").expect("http");
    let agent = Agent::new();
    agent.set_cookie("s=1; Path=/; Secure", &https);
    assert!(agent.cookies_for(&http).is_empty());
    assert_eq!(agent.cookies_for(&https), "s=1");
}

#[test]
fn httponly_cookie_cannot_be_overwritten_from_document_cookie() {
    let hops = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let server_hops = std::sync::Arc::clone(&hops);
    let server = TestServer::start(move |conn| {
        conn.read_request();
        let n = {
            let mut g = server_hops.lock().expect("hops");
            let n = *g;
            *g += 1;
            n
        };
        if n == 0 {
            conn.write_all(&canned_set_cookie("a=1; Path=/; HttpOnly", b"x"))
                .expect("set");
        } else {
            conn.write_all(&canned_ok(b"y")).expect("ok");
        }
    });
    let agent = Agent::new();
    let uri = server.url("/");
    agent.request(Method::GET, uri.clone()).send().expect("set");
    agent.set_cookie("a=stolen", &uri);
    assert!(agent.cookies_for(&uri).is_empty());
    agent.request(Method::GET, uri).send().expect("replay");
    assert_eq!(server.requests()[1].header("cookie"), Some("a=1"));
    server.assert_clean();
}

#[test]
fn same_site_fetch_sends_lax_cross_site_fetch_does_not() {
    let hops = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let server_hops = std::sync::Arc::clone(&hops);
    let server = TestServer::start(move |conn| {
        conn.read_request();
        let n = {
            let mut g = server_hops.lock().expect("hops");
            let n = *g;
            *g += 1;
            n
        };
        if n == 0 {
            conn.write_all(&canned_set_cookie("lax=1; Path=/; SameSite=Lax", b"x"))
                .expect("set");
        } else {
            conn.write_all(&canned_ok(b"y")).expect("ok");
        }
    });
    let agent = Agent::new();
    let uri = server.url("/");
    let foreign = url::Url::parse("https://evil.example/").expect("foreign");
    agent
        .request(Method::GET, uri.clone())
        .with_context(Context::Navigation)
        .send()
        .expect("set");
    agent
        .request(Method::GET, uri.clone())
        .with_context(Context::Fetch)
        .send()
        .expect("same-site fetch");
    agent
        .request(Method::GET, uri.clone())
        .with_context(Context::Fetch)
        .with_initiator(foreign)
        .send()
        .expect("cross-site fetch");
    agent
        .request(Method::GET, uri)
        .with_context(Context::Navigation)
        .send()
        .expect("nav");
    let recorded = server.requests();
    assert_eq!(recorded[1].header("cookie"), Some("lax=1"));
    assert!(recorded[2].header("cookie").is_none());
    assert_eq!(recorded[3].header("cookie"), Some("lax=1"));
    server.assert_clean();
}

#[test]
fn strict_withheld_on_cross_site_navigation_lax_only_on_safe_top_level() {
    let hops = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let server_hops = std::sync::Arc::clone(&hops);
    let server = TestServer::start(move |conn| {
        conn.read_request();
        let n = {
            let mut g = server_hops.lock().expect("hops");
            let n = *g;
            *g += 1;
            n
        };
        if n == 0 {
            conn.write_all(&canned_set_cookie(
                "s=1; Path=/; SameSite=Strict\r\nSet-Cookie: l=1; Path=/; SameSite=Lax",
                b"x",
            ))
            .expect("set");
        } else {
            conn.write_all(&canned_ok(b"y")).expect("ok");
        }
    });
    let agent = Agent::new();
    let uri = server.url("/");
    let foreign = url::Url::parse("https://evil.example/").expect("foreign");
    agent
        .request(Method::GET, uri.clone())
        .with_context(Context::Navigation)
        .send()
        .expect("set");
    agent
        .request(Method::GET, uri.clone())
        .with_context(Context::Navigation)
        .with_initiator(foreign.clone())
        .send()
        .expect("cross GET nav");
    agent
        .request(Method::POST, uri)
        .with_context(Context::Navigation)
        .with_initiator(foreign)
        .body(b"x".as_slice())
        .send()
        .expect("cross POST nav");
    let recorded = server.requests();
    assert_eq!(recorded[1].header("cookie"), Some("l=1"));
    assert!(recorded[2].header("cookie").is_none());
    server.assert_clean();
}

#[test]
fn cross_site_fetch_cannot_store_lax_cookies() {
    let server = TestServer::start(|conn| {
        conn.read_request();
        conn.write_all(&canned_set_cookie("planted=1; Path=/; SameSite=Lax", b"x"))
            .expect("set");
    });
    let agent = Agent::new();
    let uri = server.url("/");
    let foreign = url::Url::parse("https://evil.example/").expect("foreign");
    agent
        .request(Method::GET, uri.clone())
        .with_context(Context::Fetch)
        .with_initiator(foreign)
        .send()
        .expect("cross fetch");
    assert!(agent.cookies_for(&uri).is_empty());
    server.assert_clean();
}

#[test]
fn public_suffix_domain_is_rejected() {
    let uri = url::Url::parse("https://pages.github.io/app").expect("uri");
    let agent = Agent::new();
    agent.set_cookie("x=1; Path=/; Domain=github.io", &uri);
    assert!(agent.cookies_for(&uri).is_empty());
}

#[test]
fn s3_amazonaws_com_is_a_public_suffix() {
    let uri = url::Url::parse("https://evil.s3.amazonaws.com/obj").expect("uri");
    let agent = Agent::new();
    agent.set_cookie("x=1; Path=/; Domain=s3.amazonaws.com", &uri);
    assert!(
        agent.cookies_for(&uri).is_empty(),
        "Domain=s3.amazonaws.com must not store from a bucket host"
    );
}

#[test]
fn replacing_a_cookie_keeps_creation_order() {
    let uri = url::Url::parse("https://example.com/").expect("uri");
    let agent = Agent::new();
    agent.set_cookie("a=1; Path=/", &uri);
    std::thread::sleep(std::time::Duration::from_millis(2));
    agent.set_cookie("b=1; Path=/", &uri);
    agent.set_cookie("a=2; Path=/", &uri);
    assert_eq!(agent.cookies_for(&uri), "a=2; b=1");
}

#[test]
fn max_age_zero_expires_immediately() {
    let https = url::Url::parse("https://example.com/").expect("https");
    let agent = Agent::new();
    agent.set_cookie("n=v; Path=/; Max-Age=0", &https);
    assert!(agent.cookies_for(&https).is_empty());
}

#[test]
fn rfc_worked_example_sid_with_path_and_domain() {
    // RFC 6265 §3.1 / 6265bis examples: SID scoped to example.com + path /.
    let uri = url::Url::parse("https://www.example.com/").expect("uri");
    let agent = Agent::new();
    agent.set_cookie("SID=31d4d96e407aad42; Path=/; Domain=example.com", &uri);
    assert_eq!(
        agent.cookies_for(&url::Url::parse("https://www.example.com/docs").expect("sub")),
        "SID=31d4d96e407aad42"
    );
    assert!(
        agent
            .cookies_for(&url::Url::parse("https://other.example.org/").expect("other"))
            .is_empty()
    );
}

#[test]
fn same_site_none_requires_secure_and_survives_document_cookie() {
    let https = url::Url::parse("https://example.com/").expect("https");
    let http = url::Url::parse("http://example.com/").expect("http");
    let agent = Agent::new();
    agent.set_cookie("x=1; Path=/; SameSite=None", &https);
    assert!(
        agent.cookies_for(&https).is_empty(),
        "SameSite=None without Secure must not store"
    );
    agent.set_cookie("n=1; Path=/; Secure; SameSite=None", &https);
    assert_eq!(agent.cookies_for(&https), "n=1");
    assert!(
        agent.cookies_for(&http).is_empty(),
        "Secure cookie must not appear on cleartext document.cookie"
    );
}

#[test]
fn cookie_name_prefixes_enforce_secure_and_host_rules() {
    let https = url::Url::parse("https://www.example.com/app").expect("https");
    let agent = Agent::new();
    agent.set_cookie("__Secure-a=1; Path=/", &https);
    assert!(
        agent.cookies_for(&https).is_empty(),
        "__Secure- without Secure must reject"
    );
    agent.set_cookie("__Secure-a=1; Path=/; Secure", &https);
    assert_eq!(agent.cookies_for(&https), "__Secure-a=1");

    agent.set_cookie("__Host-b=1; Path=/docs; Secure", &https);
    assert!(
        !agent.cookies_for(&https).contains("__Host-b"),
        "__Host- requires Path=/"
    );
    agent.set_cookie("__Host-b=1; Path=/; Secure; Domain=example.com", &https);
    assert!(
        !agent.cookies_for(&https).contains("__Host-b"),
        "__Host- must be host-only (no Domain)"
    );
    agent.set_cookie("__Host-b=1; Path=/; Secure", &https);
    assert!(
        agent.cookies_for(&https).contains("__Host-b=1"),
        "__Host- with Secure + Path=/ must store"
    );
}

#[test]
fn expires_cookie_date_accepts_two_digit_year_and_hyphen_form() {
    let https = url::Url::parse("https://example.com/").expect("https");
    let agent = Agent::new();
    // Past rfc850-ish date → expired immediately.
    agent.set_cookie(
        "gone=1; Path=/; Expires=Wed, 09-Jun-01 10:18:14 GMT",
        &https,
    );
    assert!(agent.cookies_for(&https).is_empty());
    // Far-future IMF-fixdate still stores.
    agent.set_cookie(
        "stay=1; Path=/; Expires=Sun, 06 Nov 2094 08:49:37 GMT",
        &https,
    );
    assert_eq!(agent.cookies_for(&https), "stay=1");
    // Slash delimiters are in the cookie-date grammar; a past date must
    // expire, not fail-open into a session cookie.
    agent.set_cookie("slash=1; Path=/; Expires=09/Nov/1999 23:12:40 GMT", &https);
    assert!(
        !agent.cookies_for(&https).contains("slash="),
        "past slash-delimited Expires must not persist as a session cookie"
    );
}

#[test]
fn max_age_parse_failure_does_not_clear_prior_max_age() {
    let https = url::Url::parse("https://example.com/").expect("https");
    let agent = Agent::new();
    // If a failed Max-Age av wiped the prior one, this would become a
    // session cookie and stay visible. Keeping Max-Age=0 expires it.
    agent.set_cookie("x=1; Path=/; Max-Age=0; Max-Age=nope", &https);
    assert!(
        agent.cookies_for(&https).is_empty(),
        "failed Max-Age av must be ignored, keeping the earlier Max-Age=0"
    );
}
