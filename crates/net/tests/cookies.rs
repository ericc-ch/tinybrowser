mod common;

use common::TestServer;
use net::{Agent, Context, Method};

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
    assert_eq!(recorded[1].header("cookie"), Some("sid=1; hidden=2; lax=3"));
    assert_eq!(recorded[2].header("cookie"), Some("hidden=2; lax=3"));
    assert_eq!(agent.cookies_for(&root), "lax=3");
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
        .with_context(Context::Navigation)
        .send()
        .expect("set");
    agent
        .request(Method::GET, uri.clone())
        .with_context(Context::Fetch)
        .with_initiator(foreign.clone())
        .send()
        .expect("cross fetch");
    agent
        .request(Method::GET, uri.clone())
        .with_context(Context::Navigation)
        .with_initiator(foreign.clone())
        .send()
        .expect("cross navigation");
    agent
        .request(Method::POST, uri)
        .with_context(Context::Navigation)
        .with_initiator(foreign)
        .body(b"x")
        .send()
        .expect("cross post");

    let recorded = server.requests();
    assert!(recorded[1].header("cookie").is_none());
    assert_eq!(recorded[2].header("cookie"), Some("lax=2"));
    assert!(recorded[3].header("cookie").is_none());
    server.assert_clean();
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
}
