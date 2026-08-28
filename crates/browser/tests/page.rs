//! Page-thread loop: host timers and `spawn_blocking` fetch against loopback.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use browser::{Agent, Page, PageError, PageEvent, ScriptFailure};
use dom::NodeKind;

fn slow_ok_server(delay: Duration) -> String {
    serve_once(delay, "200 OK", &[], b"ok")
}

fn serve_once(delay: Duration, status: &str, extra_headers: &[&str], body: &[u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let status = status.to_owned();
    let extra_headers: Vec<String> = extra_headers.iter().map(|h| (*h).to_owned()).collect();
    let body = body.to_vec();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        let mut head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for header in extra_headers {
            head.push_str(&header);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).expect("write head");
        stream.write_all(&body).expect("write body");
    });
    format!("http://{addr}/")
}

fn capture_request_target() -> (String, std::sync::Arc<std::sync::Mutex<Option<String>>>) {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let flag = captured.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).expect("read");
        let head = String::from_utf8_lossy(&buf[..n]);
        let target = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("")
            .to_owned();
        *flag.lock().expect("lock") = Some(target);
        let body = b"ok";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(resp.as_bytes()).expect("write");
        stream.write_all(body).expect("body");
    });
    (format!("http://{addr}"), captured)
}

fn element_text(dom: &dom::Dom, id: dom::NodeId) -> String {
    let mut out = String::new();
    let Some(kids) = dom.children(id) else {
        return out;
    };
    for kid in kids {
        if let Some(NodeKind::Text { data }) = dom.get(*kid).map(|n| n.kind()) {
            out.push_str(data);
        }
    }
    out
}

#[test]
fn two_pages_share_one_agent_cookie_jar() {
    let agent = Agent::new();
    let mut a = Page::with_agent(agent.clone());
    let mut b = Page::with_agent(agent);
    a.set_document_url("http://example.test/")
        .expect("absolute url");
    b.set_document_url("http://example.test/")
        .expect("absolute url");
    a.set_document_cookie("a=1");
    assert_eq!(b.document_cookie(), "a=1");
}

#[test]
fn relative_fetch_resolves_against_base_href() {
    let (origin, captured) = capture_request_target();
    let mut page = Page::new();
    page.set_document_url(&format!("{origin}/dir/page.html"))
        .expect("origin");
    page.load_html("<base href=\"/app/\">");
    page.start_fetch("next").expect("relative");
    page.run();
    let target = captured.lock().expect("lock").clone().expect("saw request");
    assert_eq!(target, "/app/next");
}

#[test]
fn js_fetch_resolves_against_base_href() {
    let (origin, captured) = capture_request_target();
    let mut page = Page::new();
    page.set_document_url(&format!("{origin}/dir/page.html"))
        .expect("origin");
    page.load_html("<base href=\"/app/\">");
    page.eval("fetch('next')").expect("js fetch");
    page.run();
    let target = captured.lock().expect("lock").clone().expect("saw request");
    assert_eq!(target, "/app/next");
}

#[test]
fn goto_parses_html_and_stores_content_language() {
    let body = b"<!DOCTYPE html><p lang=en>hi</p>";
    let url = serve_once(Duration::ZERO, "200 OK", &["Content-Language: fr"], body);
    let mut page = Page::new();
    page.goto(&url).expect("goto");
    page.run();
    assert_eq!(page.content_language(), Some("fr"));
    assert_eq!(page.document_url(), url);
    let parsed = page.parsed().expect("parsed");
    assert_eq!(parsed.dom.document_language(), Some("fr"));
    let p = parsed
        .dom
        .select_first(parsed.dom.document(), "p")
        .expect("select")
        .expect("p");
    assert_eq!(element_text(&parsed.dom, p), "hi");
}

#[test]
fn goto_comma_separated_content_language_is_none() {
    let body = b"<!DOCTYPE html><p>hi</p>";
    let url = serve_once(
        Duration::ZERO,
        "200 OK",
        &["Content-Language: en, fr"],
        body,
    );
    let mut page = Page::new();
    page.goto(&url).expect("goto");
    page.run();
    assert_eq!(page.content_language(), None);
}

#[test]
fn goto_connection_refused_is_fetch_failed() {
    let mut page = Page::new();
    page.goto("http://127.0.0.1:1/").expect("queued");
    page.run();
    assert_eq!(page.events(), &[PageEvent::FetchFailed]);
    assert!(page.parsed().is_none());
}

#[test]
fn eval_throw_is_script_error() {
    let mut page = Page::new();
    let err = page
        .eval("throw new Error('boom')")
        .expect_err("throw should fail eval");
    assert!(
        matches!(err, PageError::Script(ScriptFailure::Engine { .. })),
        "expected engine script failure, got {err:?}"
    );
}

#[test]
fn eval_completion_value_is_stringified() {
    let mut page = Page::new();
    assert_eq!(page.eval("1 + 1").expect("eval"), "2");
}

#[test]
fn eval_settimeout_and_document_cookie() {
    let mut page = Page::new();
    page.set_document_url("http://example.test/")
        .expect("origin");
    page.eval("document.cookie = 'a=1'").expect("cookie set");
    assert_eq!(page.document_cookie(), "a=1");
    page.eval("setTimeout(function() { globalThis.hit = 1; }, 20)")
        .expect("timer");
    page.run();
    let hit = page.eval("String(globalThis.hit)").expect("read hit");
    assert_eq!(hit, "1");
}

#[test]
fn eval_fetch_timer_runs_while_js_fetch_waits() {
    let url = slow_ok_server(Duration::from_millis(400));
    let mut page = Page::new();
    page.eval(&format!(
        "globalThis.got = 0; setTimeout(function() {{ globalThis.hit = 1; }}, 30); fetch('{url}').then(function(r) {{ globalThis.got = r.status; }})"
    ))
    .expect("schedule");
    page.run();
    assert_eq!(
        page.events(),
        &[PageEvent::Timer(1), PageEvent::Fetch { status: 200 },]
    );
    assert_eq!(page.eval("String(globalThis.hit)").expect("hit"), "1");
    assert_eq!(page.eval("String(globalThis.got)").expect("status"), "200");
}

#[test]
fn eval_fetch_body_is_readable() {
    let url = slow_ok_server(Duration::ZERO);
    let mut page = Page::new();
    page.eval(&format!(
        "globalThis.body = ''; fetch('{url}').then(function(r) {{ return r.text(); }}).then(function(t) {{ globalThis.body = t; }})"
    ))
    .expect("fetch");
    page.run();
    assert_eq!(page.eval("String(globalThis.body)").expect("body"), "ok");
}

#[test]
fn eval_fetch_inside_settimeout_still_dials() {
    let url = slow_ok_server(Duration::from_millis(30));
    let mut page = Page::new();
    page.eval(&format!(
        "globalThis.got = 0; setTimeout(function() {{ fetch('{url}').then(function(r) {{ globalThis.got = r.status; }}); }}, 10)"
    ))
    .expect("schedule");
    page.run();
    assert_eq!(
        page.events(),
        &[PageEvent::Timer(1), PageEvent::Fetch { status: 200 },]
    );
    assert_eq!(page.eval("String(globalThis.got)").expect("read"), "200");
}

#[test]
fn eval_fetch_rejects_on_connection_refused() {
    let mut page = Page::new();
    page.eval(
        "globalThis.ok = 0; fetch('http://127.0.0.1:1/').then(function() {}, function() { globalThis.ok = 1; })",
    )
    .expect("fetch");
    page.run();
    assert_eq!(page.eval("String(globalThis.ok)").expect("read"), "1");
    assert_eq!(page.events(), &[PageEvent::FetchFailed]);
}

#[test]
fn js_fetch_non_http_is_fetch_failed() {
    let mut page = Page::new();
    page.eval(
        "globalThis.ok = 0; fetch('file:///etc/passwd').then(function() {}, function() { globalThis.ok = 1; })",
    )
    .expect("fetch");
    page.run();
    assert_eq!(page.eval("String(globalThis.ok)").expect("read"), "1");
    assert_eq!(page.events(), &[PageEvent::FetchFailed]);
}

#[test]
fn load_html_does_not_settle_stale_js_fetch() {
    let url = slow_ok_server(Duration::from_millis(200));
    let mut page = Page::new();
    page.eval(&format!(
        "fetch('{url}').then(function(r) {{ globalThis.got = r.status; }})"
    ))
    .expect("fetch");
    page.load_html("<p>x</p>");
    page.eval("globalThis.got = 'seed'").expect("new realm");
    page.run();
    assert_eq!(page.eval("globalThis.got").expect("got"), "seed");
}

#[test]
fn timer_callback_throw_is_script_failed() {
    let mut page = Page::new();
    page.eval("setTimeout(function() { throw new Error('boom'); }, 10)")
        .expect("timer");
    page.run();
    assert!(
        page.events().contains(&PageEvent::ScriptFailed),
        "events: {:?}",
        page.events()
    );
}

#[test]
fn load_html_starts_a_new_js_realm() {
    let mut page = Page::new();
    page.eval("globalThis.secret = 1").expect("seed");
    page.load_html("<p>x</p>");
    let secret = page.eval("typeof globalThis.secret").expect("read");
    assert_eq!(secret, "undefined");
}

#[test]
fn start_fetch_rejects_non_http_schemes() {
    let mut page = Page::new();
    assert!(matches!(
        page.start_fetch("file:///etc/passwd"),
        Err(PageError::InvalidUrl { .. })
    ));
}

#[test]
fn host_timer_runs_while_fetch_waits_on_spawn_blocking() {
    let url = slow_ok_server(Duration::from_millis(400));
    let mut page = Page::new();
    let timer = page.schedule_timer(Duration::from_millis(30));
    page.start_fetch(&url).expect("absolute loopback url");
    page.run();
    assert_eq!(
        page.events(),
        &[PageEvent::Timer(timer), PageEvent::Fetch { status: 200 },]
    );
}

#[test]
fn load_html_puts_the_tree_on_the_page() {
    let mut page = Page::new();
    page.load_html("<p>hi</p>");
    let parsed = page.parsed().expect("loaded");
    let p = parsed
        .dom
        .select_first(parsed.dom.document(), "p")
        .expect("select")
        .expect("p");
    assert_eq!(element_text(&parsed.dom, p), "hi");
}
