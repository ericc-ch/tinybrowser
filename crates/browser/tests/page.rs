use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use browser::{Agent, Page, PageError, PageEvent, ScriptFailure};
use dom::NodeKind;

fn read_target(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let mut head = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).expect("request read");
        assert_ne!(read, 0, "peer closed before request head");
        head.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8_lossy(&head)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("request target")
        .to_owned()
}

fn respond(stream: &mut TcpStream, headers: &[&str], body: &[u8]) {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for header in headers {
        response.push_str(header);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    stream
        .write_all(response.as_bytes())
        .expect("response head");
    stream.write_all(body).expect("response body");
}

fn accept_before(listener: &TcpListener, deadline: Instant) -> TcpStream {
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "timed out waiting for request");
                thread::yield_now();
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    }
}

fn element_text(dom: &dom::Dom, id: dom::NodeId) -> String {
    dom.children(id)
        .into_iter()
        .flatten()
        .filter_map(|child| match dom.get(*child).map(|node| node.kind()) {
            Some(NodeKind::Text { data }) => Some(data.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn navigation_parsing_cookies_and_relative_js_fetch_form_one_journey() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut navigation, _) = listener.accept().expect("navigation");
        assert_eq!(read_target(&mut navigation), "/start");
        respond(
            &mut navigation,
            &["Content-Language: fr", "Set-Cookie: sid=1; Path=/"],
            b"<!doctype html><base href=\"/app/\"><p id=loaded>hi</p>",
        );

        let (mut fetch, _) = listener.accept().expect("fetch");
        assert_eq!(read_target(&mut fetch), "/app/next");
        respond(&mut fetch, &[], b"payload");
    });

    let agent = Agent::new();
    let mut page = Page::with_agent(agent.clone());
    page.goto(&format!("http://{addr}/start")).expect("goto");
    page.run();

    assert_eq!(page.content_language(), Some("fr"));
    assert_eq!(page.document_cookie(), "sid=1");
    let parsed = page.parsed().expect("parsed navigation");
    let paragraph = parsed
        .dom
        .select_first(parsed.dom.document(), "#loaded")
        .expect("selector")
        .expect("paragraph");
    assert_eq!(element_text(&parsed.dom, paragraph), "hi");

    page.eval(
        "globalThis.body = ''; fetch('next').then(function(response) { return response.text(); }).then(function(text) { globalThis.body = text; });",
    )
    .expect("fetch script");
    page.run();
    assert_eq!(page.eval("globalThis.body").expect("body"), "payload");
    assert_eq!(
        page.events(),
        &[
            PageEvent::Fetch { status: 200 },
            PageEvent::Fetch { status: 200 }
        ]
    );

    let mut sibling = Page::with_agent(agent);
    sibling
        .set_document_url(&format!("http://{addr}/elsewhere"))
        .expect("document URL");
    assert_eq!(sibling.document_cookie(), "sid=1");
    server.join().expect("server");
}

#[test]
fn page_loop_correlates_more_than_one_batch_of_fetches() {
    const REQUESTS: usize = 9;
    const FIRST_BATCH: usize = 8;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let addr = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut first_batch = Vec::new();
        for _ in 0..FIRST_BATCH {
            let mut stream = accept_before(&listener, deadline);
            let target = read_target(&mut stream);
            first_batch.push((target, stream));
        }
        for (target, mut stream) in first_batch.into_iter().rev() {
            respond(&mut stream, &[], target.trim_start_matches('/').as_bytes());
        }
        let mut last = accept_before(&listener, deadline);
        let target = read_target(&mut last);
        respond(&mut last, &[], target.trim_start_matches('/').as_bytes());
    });

    let agent = net::AgentBuilder::new()
        .timeout_global(Duration::from_secs(3))
        .build();
    let mut page = Page::with_agent(agent);
    let origin = format!("http://{addr}");
    page.set_document_url(&format!("{origin}/"))
        .expect("origin");
    page.eval(&format!(
        "globalThis.results = []; globalThis.timerHit = false; \
         for (let i = 0; i < {REQUESTS}; i++) {{ \
           ((slot) => fetch('{origin}/' + slot).then((response) => response.text()).then((body) => {{ results[slot] = body; }}))(i); \
         }} \
         setTimeout(() => {{ globalThis.timerHit = true; }}, 0);"
    ))
    .expect("schedule work");
    page.run();

    assert_eq!(
        page.eval("String(globalThis.timerHit)").expect("timer"),
        "true"
    );
    assert_eq!(
        page.eval("globalThis.results.join(',')").expect("results"),
        "0,1,2,3,4,5,6,7,8"
    );
    assert_eq!(
        page.events()
            .iter()
            .filter(|event| matches!(event, PageEvent::Fetch { status: 200 }))
            .count(),
        REQUESTS
    );
    assert!(matches!(page.events().first(), Some(PageEvent::Timer(_))));
    server.join().expect("server");
}

#[test]
fn realm_replacement_and_failed_jobs_drain_without_leaking_work() {
    let mut page = Page::new();
    let error = page
        .eval(
            "Promise.resolve().then(() => { globalThis.microtask = true; }); \
             setTimeout(() => { globalThis.timer = true; }, 0); \
             throw Error('boom');",
        )
        .expect_err("script throws");
    assert!(matches!(
        error,
        PageError::Script(ScriptFailure::Engine { .. })
    ));
    assert_eq!(
        page.eval("globalThis.microtask").expect("microtask"),
        "true"
    );
    page.run();
    assert_eq!(page.eval("globalThis.timer").expect("timer"), "true");

    page.eval("globalThis.secret = 1").expect("old realm");
    page.goto("http://127.0.0.1:1/").expect("queued navigation");
    page.load_html("<p>local</p>");
    page.run();
    assert_eq!(
        page.eval("typeof globalThis.secret").expect("new realm"),
        "undefined"
    );
    assert!(!page.events().contains(&PageEvent::FetchFailed));

    page.eval(
        "globalThis.done = false; \
         fetch('file:///one').catch(() => fetch('file:///two')).catch(() => { globalThis.done = true; });",
    )
    .expect("rejected chain");
    page.run();
    assert_eq!(page.eval("globalThis.done").expect("drained"), "true");
    assert_eq!(
        page.events()
            .iter()
            .filter(|event| **event == PageEvent::FetchFailed)
            .count(),
        2
    );
}
