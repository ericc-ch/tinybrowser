use super::common::TestServer;
use net::{Agent, InitiatorKind, Method};

fn response(set_cookies: &[&str], body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .into_bytes();
    for cookie in set_cookies {
        out.extend_from_slice(format!("Set-Cookie: {cookie}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
}

#[test]
fn http_cookies_scope_by_path_and_visibility() {
    let request_number = std::sync::Arc::new(std::sync::Mutex::new(0_u8));
    let counter = std::sync::Arc::clone(&request_number);
    let server = TestServer::start(move |connection| {
        let request = connection.read_request();
        let mut number = counter.lock().expect("counter");
        let response = match *number {
            0 => response(
                &[
                    "sid=1; Path=/docs",
                    "hidden=2; Path=/; HttpOnly",
                    "lax=3; Path=/; SameSite=Lax",
                    "secure=4; Path=/; Secure",
                ],
                b"set",
            ),
            1 => response(&[], b"docs"),
            2 => response(&[], b"other"),
            _ => panic!("unexpected request {}", request.target),
        };
        *number += 1;
        connection.write_all(&response).expect("response");
    });

    let agent = Agent::new();
    let root = server.url("/");
    agent
        .request(Method::GET, root.clone())
        .send()
        .expect("set cookies");
    let docs = server.url("/docs/guide");
    agent.request(Method::GET, docs).send().expect("scoped");
    agent
        .request(Method::GET, server.url("/other"))
        .send()
        .expect("unscoped");

    let recorded = server.requests();
    assert_eq!(
        recorded[1].header("cookie"),
        Some("sid=1; hidden=2; lax=3; secure=4")
    );
    assert_eq!(
        recorded[2].header("cookie"),
        Some("hidden=2; lax=3; secure=4")
    );
    assert_eq!(agent.cookies_for(&root), "lax=3; secure=4");
    server.assert_clean();
}

#[test]
fn same_site_context_controls_cross_site_request_cookies() {
    let request_number = std::sync::Arc::new(std::sync::Mutex::new(0_u8));
    let counter = std::sync::Arc::clone(&request_number);
    let server = TestServer::start(move |connection| {
        connection.read_request();
        let mut number = counter.lock().expect("counter");
        let response = if *number == 0 {
            response(
                &[
                    "strict=1; Path=/; SameSite=Strict",
                    "lax=2; Path=/; SameSite=Lax",
                ],
                b"set",
            )
        } else {
            response(&[], b"ok")
        };
        *number += 1;
        connection.write_all(&response).expect("response");
    });

    let agent = Agent::new();
    let uri = server.url("/");
    let foreign = url::Url::parse("https://evil.example/").expect("foreign");
    agent
        .request(Method::GET, uri.clone())
        .with_initiator_kind(InitiatorKind::Navigation)
        .send()
        .expect("set");
    agent
        .request(Method::GET, uri.clone())
        .with_initiator_kind(InitiatorKind::Fetch)
        .with_initiator(foreign.clone())
        .send()
        .expect("cross fetch");
    agent
        .request(Method::GET, uri.clone())
        .with_initiator_kind(InitiatorKind::Navigation)
        .with_initiator(foreign.clone())
        .send()
        .expect("cross navigation");
    agent
        .request(Method::POST, uri)
        .with_initiator_kind(InitiatorKind::Navigation)
        .with_initiator(foreign)
        .body(b"x")
        .send()
        .expect("cross post");
    agent
        .request(Method::GET, server.url("/"))
        .with_initiator(url::Url::parse("https://evil.example/").expect("foreign"))
        .send()
        .expect("default navigation");

    let recorded = server.requests();
    assert!(recorded[1].header("cookie").is_none());
    assert_eq!(recorded[2].header("cookie"), Some("lax=2"));
    assert!(recorded[3].header("cookie").is_none());
    assert_eq!(recorded[4].header("cookie"), Some("lax=2"));
    server.assert_clean();
}

#[test]
fn cookie_max_age_and_global_eviction_follow_storage_rules() {
    let agent = Agent::new();
    let url = url::Url::parse("https://max-age.example/").expect("max-age URL");
    agent.set_cookie("plus=1; Path=/; Max-Age=+0", &url);
    assert_eq!(agent.cookies_for(&url), "plus=1");

    let oldest = url::Url::parse("https://oldest.example/").expect("oldest URL");
    agent.set_cookie("secure=1; Path=/; Secure", &oldest);
    for domain in 0..60 {
        let domain =
            url::Url::parse(&format!("https://d{domain}.example/")).expect("quota domain URL");
        for cookie in 0..50 {
            agent.set_cookie(&format!("c{cookie}={cookie}; Path=/"), &domain);
        }
    }
    assert!(agent.cookies_for(&oldest).is_empty());
}

#[test]
fn cookie_security_prefixes_expiry_and_public_suffixes_are_enforced() {
    let https = url::Url::parse("https://www.example.com/app").expect("https");
    let http = url::Url::parse("http://www.example.com/app").expect("http");
    let agent = Agent::new();

    agent.set_cookie("__Secure-a=1; Path=/", &https);
    agent.set_cookie("__Secure-a=2; Path=/; Secure", &https);
    agent.set_cookie("__Host-b=1; Path=/docs; Secure", &https);
    agent.set_cookie("__Host-b=2; Path=/; Secure; Domain=example.com", &https);
    agent.set_cookie("__Host-b=3; Path=/; Secure", &https);
    agent.set_cookie("gone=1; Path=/; Max-Age=0", &https);
    agent.set_cookie("old=1; Path=/; Expires=Wed, 09-Jun-01 10:18:14 GMT", &https);
    agent.set_cookie("public=1; Path=/; Domain=com", &https);

    assert_eq!(agent.cookies_for(&https), "__Secure-a=2; __Host-b=3");
    assert!(agent.cookies_for(&http).is_empty());

    let s3 = url::Url::parse("https://evil.s3.amazonaws.com/obj").expect("s3");
    agent.set_cookie("bucket=1; Path=/; Domain=s3.amazonaws.com", &s3);
    assert!(agent.cookies_for(&s3).is_empty());

    let pages = url::Url::parse("https://github.io/").expect("pages");
    agent.set_cookie("id=1; Path=/; Domain=github.io", &pages);
    assert_eq!(agent.cookies_for(&pages), "id=1");
    assert!(
        agent
            .cookies_for(&url::Url::parse("https://foo.github.io/").expect("sub"))
            .is_empty()
    );

    agent.set_cookie("__Secure-x; Path=/; Secure", &https);
    assert_eq!(agent.cookies_for(&https), "__Secure-a=2; __Host-b=3");

    let localhost = url::Url::parse("http://localhost/app").expect("localhost");
    agent.set_cookie("dev=1; Path=/; Secure", &localhost);
    assert_eq!(agent.cookies_for(&localhost), "dev=1");

    let ip = url::Url::parse("http://127.0.0.1/").expect("ip");
    agent.set_cookie("a=1; Path=/; Domain=127.0.0.1", &ip);
    assert_eq!(agent.cookies_for(&ip), "a=1");
    agent.set_cookie("b=1; Path=/; Domain=192.0.2.1", &ip);
    assert_eq!(agent.cookies_for(&ip), "a=1");

    let idn = url::Url::parse("https://xn--mnchen-3ya.de/").expect("idn");
    agent.set_cookie("c=1; Path=/; Domain=münchen.de", &idn);
    assert_eq!(agent.cookies_for(&idn), "c=1");

    let quota = url::Url::parse("https://quota.example/").expect("quota");
    for i in 0..51 {
        agent.set_cookie(&format!("n{i}={i}; Path=/"), &quota);
    }
    let listed = agent.cookies_for(&quota);
    assert_eq!(listed.split("; ").count(), 50);
    assert!(!listed.contains("n0="));
    assert!(listed.contains("n50="));
}
