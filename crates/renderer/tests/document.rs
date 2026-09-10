use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use dom::NodeKind;
use net::{Agent, AgentBuilder, InitiatorKind, Method};
use renderer::{
    BrowserServices, DialKind, DialOutcome, DialRequest, Document, Mount, ScriptFailure, TabError,
    TabEvent,
};
use url::Url;

/// Test services: dials through `net` and keeps one cookie jar.
struct TestServices {
    agent: Agent,
}

impl TestServices {
    fn new() -> Self {
        Self {
            agent: Agent::new(),
        }
    }

    fn with_agent(agent: Agent) -> Self {
        Self { agent }
    }

    /// Browser-side navigation dial (what the browser does before `mount`).
    fn navigate(&self, url: &str) -> Option<DialOutcome> {
        let url = Url::parse(url).ok()?;
        let response = self
            .agent
            .request(Method::GET, url)
            .with_initiator_kind(InitiatorKind::Navigation)
            .send()
            .ok()?;
        let status = response.status();
        let final_url = response.final_url().to_string();
        let content_language = response
            .headers()
            .get("content-language")
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(content_language_tag);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .map(str::to_owned);
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk) = body.read_chunk().ok()? {
            bytes.extend_from_slice(&chunk);
        }
        Some(DialOutcome {
            status,
            final_url,
            content_type,
            content_language,
            body: bytes,
        })
    }
}

impl BrowserServices for TestServices {
    fn dial(&self, request: &DialRequest) -> Option<DialOutcome> {
        let url = Url::parse(&request.url).ok()?;
        let initiator_kind = match request.kind {
            DialKind::JsFetch | DialKind::ClassicScript => InitiatorKind::Fetch,
        };
        let initiator = Url::parse(&request.initiator).ok()?;
        let response = self
            .agent
            .request(Method::GET, url)
            .with_initiator_kind(initiator_kind)
            .with_initiator(initiator)
            .send()
            .ok()?;
        let status = response.status();
        let final_url = response.final_url().to_string();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .map(str::to_owned);
        let mut body = Vec::new();
        if request.read_body {
            let mut response_body = response.into_body();
            while let Some(chunk) = response_body.read_chunk().ok()? {
                body.extend_from_slice(&chunk);
            }
        }
        Some(DialOutcome {
            status,
            final_url,
            content_type,
            content_language: None,
            body,
        })
    }

    fn cookies_for(&self, url: &Url) -> String {
        self.agent.cookies_for(url)
    }

    fn set_cookie(&self, value: &str, url: &Url) {
        self.agent.set_cookie(value, url);
    }

    fn mark_dirty(&self) {}
}

fn document() -> (Document, Arc<TestServices>) {
    let services = Arc::new(TestServices::new());
    (Document::new(services.clone()), services)
}

fn goto(document: &mut Document, services: &TestServices, url: &str) {
    let outcome = services.navigate(url).expect("navigation dial");
    document.mount(&Mount {
        url: outcome.final_url,
        content_type: outcome.content_type,
        content_language: outcome.content_language,
        body: outcome.body,
    });
}

/// One `Content-Language` tag, or `None` when the header lists several.
fn content_language_tag(raw: &str) -> Option<String> {
    let mut tags = raw
        .split(',')
        .map(|part| part.split(';').next().unwrap_or(part).trim())
        .filter(|tag| !tag.is_empty());
    let first = tags.next()?.to_owned();
    if tags.next().is_some() {
        return None;
    }
    Some(first)
}

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

    let services = Arc::new(TestServices::with_agent(Agent::new()));
    let mut doc = Document::new(services.clone());
    goto(&mut doc, &services, &format!("http://{addr}/start"));
    doc.run();

    assert_eq!(doc.content_language(), Some("fr"));
    assert_eq!(doc.document_cookie(), "sid=1");
    {
        let parsed = doc.parsed().expect("parsed navigation");
        let paragraph = parsed
            .dom
            .select_first(parsed.dom.document(), "#loaded")
            .expect("selector")
            .expect("paragraph");
        assert_eq!(element_text(&parsed.dom, paragraph), "hi");
    }

    doc.eval(
        "globalThis.body = ''; fetch('next').then(function(response) { return response.text(); }).then(function(text) { globalThis.body = text; });",
    )
    .expect("fetch script");
    doc.run();
    assert_eq!(doc.eval("globalThis.body").expect("body"), "payload");
    // Navigation's own Fetch event belongs to the browser process now; the renderer
    // reports the document load and the subresource fetch.
    assert_eq!(
        doc.events(),
        &[TabEvent::Load, TabEvent::Fetch { status: 200 }]
    );

    let sibling_services = Arc::new(TestServices::with_agent(services.agent.clone()));
    let mut sibling = Document::new(sibling_services);
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

    let agent = AgentBuilder::new()
        .timeout_global(Duration::from_secs(3))
        .build();
    let services = Arc::new(TestServices::with_agent(agent));
    let mut doc = Document::new(services);
    let origin = format!("http://{addr}");
    doc.set_document_url(&format!("{origin}/")).expect("origin");
    doc.eval(&format!(
        "globalThis.results = []; globalThis.timerHit = false; \
         for (let i = 0; i < {REQUESTS}; i++) {{ \
           ((slot) => fetch('{origin}/' + slot).then((response) => response.text()).then((body) => {{ results[slot] = body; }}))(i); \
         }} \
         setTimeout(() => {{ globalThis.timerHit = true; }}, 0);"
    ))
    .expect("schedule work");
    doc.run();

    assert_eq!(
        doc.eval("String(globalThis.timerHit)").expect("timer"),
        "true"
    );
    assert_eq!(
        doc.eval("globalThis.results.join(',')").expect("results"),
        "0,1,2,3,4,5,6,7,8"
    );
    assert_eq!(
        doc.events()
            .iter()
            .filter(|event| matches!(event, TabEvent::Fetch { status: 200 }))
            .count(),
        REQUESTS
    );
    assert!(matches!(doc.events().first(), Some(TabEvent::Timer(_))));
    server.join().expect("server");
}

#[test]
fn realm_replacement_and_failed_jobs_drain_without_leaking_work() {
    let (mut doc, _host) = document();
    let error = doc
        .eval(
            "Promise.resolve().then(() => { globalThis.microtask = true; }); \
             setTimeout(() => { globalThis.timer = true; }, 0); \
             throw Error('boom');",
        )
        .expect_err("script throws");
    assert!(matches!(
        error,
        TabError::Script(ScriptFailure::Engine { .. })
    ));
    assert_eq!(doc.eval("globalThis.microtask").expect("microtask"), "true");
    doc.run();
    assert_eq!(doc.eval("globalThis.timer").expect("timer"), "true");

    doc.eval("globalThis.secret = 1").expect("old realm");
    doc.load_html("<p>local</p>");
    doc.run();
    assert_eq!(
        doc.eval("typeof globalThis.secret").expect("new realm"),
        "undefined"
    );
    assert!(!doc.events().contains(&TabEvent::FetchFailed));

    doc.eval(
        "globalThis.done = false; \
         fetch('file:///one').catch(() => fetch('file:///two')).catch(() => { globalThis.done = true; });",
    )
    .expect("rejected chain");
    doc.run();
    assert_eq!(doc.eval("globalThis.done").expect("drained"), "true");
    assert_eq!(
        doc.events()
            .iter()
            .filter(|event| **event == TabEvent::FetchFailed)
            .count(),
        2
    );
}

#[test]
fn classic_scripts_run_and_window_load_fires() {
    let (mut doc, _host) = document();
    doc.load_html(
        r#"<!doctype html>
<title>t</title>
<body></body>
<script>
window.scriptRan = true;
implicit = 1;
window.sameBody = document.body === document.getElementsByTagName('body')[0];
window.addEventListener("load", function() { window.loadFired = true; });
</script>"#,
    );
    assert_eq!(
        doc.eval("String(window.scriptRan)").expect("script"),
        "true"
    );
    assert_eq!(doc.eval("String(implicit)").expect("sloppy"), "1");
    assert_eq!(
        doc.eval("String(window.sameBody)").expect("identity"),
        "true"
    );
    assert_eq!(doc.eval("String(window.loadFired)").expect("load"), "true");
    assert_eq!(
        doc.eval("String(window.parent === window && window.top === window)")
            .expect("top window"),
        "true"
    );
    assert_eq!(
        doc.eval("document.getElementsByTagName('title')[0].firstChild.data")
            .expect("title"),
        "t"
    );
    assert_eq!(
        doc.eval("document.readyState").expect("readyState"),
        "complete"
    );
}

#[test]
fn parser_blocking_script_observes_and_mutates_the_partial_document() {
    let (mut doc, _host) = document();
    doc.load_html(
        r#"<!doctype html>
<head><script>
window.bodyWasMissing = document.body === null;
var marker = document.createElement("meta");
marker.id = "made-while-parsing";
document.documentElement.appendChild(marker);
document.write('<meta id="written">');
</script></head>
<body><p>later</p></body>"#,
    );

    assert_eq!(
        doc.eval("String(window.bodyWasMissing)").expect("body"),
        "true"
    );
    assert_eq!(
        doc.eval("String(document.getElementById('made-while-parsing') !== null)")
            .expect("mutation"),
        "true"
    );
    assert_eq!(
        doc.eval("String(document.getElementById('written') !== null)")
            .expect("document.write"),
        "true"
    );
    assert_eq!(doc.eval("document.readyState").expect("state"), "complete");
}

#[test]
fn navigation_decodes_bytes_before_tokenization() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut navigation, _) = listener.accept().expect("navigation");
        let _target = read_target(&mut navigation);
        respond(
            &mut navigation,
            &["Content-Type: text/html; charset=windows-1252"],
            b"<!doctype html><p id=value>\x80</p>",
        );
    });
    let services = Arc::new(TestServices::new());
    let mut doc = Document::new(services.clone());
    goto(&mut doc, &services, &format!("http://{addr}/"));
    doc.run_until_load();

    assert_eq!(
        doc.eval("document.getElementById('value').firstChild.data")
            .expect("decoded text"),
        "€"
    );
    server.join().expect("server");
}

#[test]
fn external_classic_scripts_run_before_load() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut navigation, _) = listener.accept().expect("navigation");
        assert_eq!(read_target(&mut navigation), "/doc");
        respond(
            &mut navigation,
            &["Content-Type: text/html"],
            br#"<!doctype html>
<script src="/lib.js"></script>
<script>
window.addEventListener("load", function() { window.loadSaw = window.fromLib; });
</script>"#,
        );
        let (mut script, _) = listener.accept().expect("script");
        assert_eq!(read_target(&mut script), "/lib.js");
        respond(
            &mut script,
            &["Content-Type: text/javascript"],
            b"window.fromLib = 7;",
        );
    });

    let services = Arc::new(TestServices::new());
    let mut doc = Document::new(services.clone());
    goto(&mut doc, &services, &format!("http://{addr}/doc"));
    doc.run();
    assert_eq!(doc.eval("String(window.fromLib)").expect("lib"), "7");
    assert_eq!(doc.eval("String(window.loadSaw)").expect("load"), "7");
    server.join().expect("server");
}

#[test]
fn navigation_load_does_not_wait_for_host_timers() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut navigation, _) = listener.accept().expect("navigation");
        assert_eq!(read_target(&mut navigation), "/doc");
        respond(
            &mut navigation,
            &["Content-Type: text/html"],
            br#"<!doctype html>
<p id="a.b">x</p>
<script>
window.early = true;
setTimeout(function() { window.late = true; }, 30000);
</script>"#,
        );
    });

    let services = Arc::new(TestServices::new());
    let mut doc = Document::new(services.clone());
    goto(&mut doc, &services, &format!("http://{addr}/doc"));
    let started = Instant::now();
    doc.run_until_load();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "load waited for services timers"
    );
    assert_eq!(doc.eval("String(window.early)").expect("early"), "true");
    assert_eq!(doc.eval("typeof window.late").expect("late"), "undefined");
    assert_eq!(
        doc.eval("document.readyState").expect("readyState"),
        "complete"
    );
    assert_eq!(
        doc.eval("document.getElementById('a.b').firstChild.data")
            .expect("id"),
        "x"
    );
    server.join().expect("server");
}

#[test]
fn run_until_load_does_not_wait_for_unrelated_fetch() {
    let slow = TcpListener::bind("127.0.0.1:0").expect("slow bind");
    let slow_addr = slow.local_addr().expect("slow addr");
    let page_listener = TcpListener::bind("127.0.0.1:0").expect("doc bind");
    let page_addr = page_listener.local_addr().expect("doc addr");
    let release = Arc::new(AtomicBool::new(false));
    let slow_flag = Arc::clone(&release);
    let slow_server = thread::spawn(move || {
        let (mut stream, _) = slow.accept().expect("slow accept");
        let _ = read_target(&mut stream);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !slow_flag.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "slow fetch never released");
            thread::sleep(Duration::from_millis(10));
        }
        respond(&mut stream, &[], b"slow");
    });
    let page_server = thread::spawn(move || {
        let (mut stream, _) = page_listener.accept().expect("doc accept");
        let _ = read_target(&mut stream);
        let html = format!(
            "<!doctype html><script>fetch('http://{slow_addr}/slow').then(function() {{ window.slowDone = true; }});</script>"
        );
        respond(&mut stream, &["Content-Type: text/html"], html.as_bytes());
    });

    let services = Arc::new(TestServices::new());
    let mut doc = Document::new(services.clone());
    goto(&mut doc, &services, &format!("http://{page_addr}/"));
    let started = Instant::now();
    doc.run_until_load();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "load waited for unrelated fetch: {:?}",
        started.elapsed()
    );
    assert_eq!(
        doc.eval("typeof window.slowDone").expect("slow"),
        "undefined"
    );
    release.store(true, Ordering::SeqCst);
    doc.run();
    assert_eq!(doc.eval("String(window.slowDone)").expect("done"), "true");
    slow_server.join().expect("slow server");
    page_server.join().expect("doc server");
}

#[test]
fn created_documents_are_second_trees() {
    let (mut doc, _host) = document();
    doc.load_html("<!doctype html><title>main</title><body><p id=here>x</p></body>");

    assert_eq!(
        doc.eval(
            "var secondary = document.implementation.createHTMLDocument('T');\
             String(secondary.contentType)"
        )
        .expect("contentType"),
        "text/html"
    );
    assert_eq!(
        doc.eval(
            "secondary.body.appendChild(secondary.createElement('p')).textContent = 'second';\
             secondary.title"
        )
        .expect("title"),
        "T"
    );
    assert_eq!(
        doc.eval("String(secondary.body.firstChild.textContent)")
            .expect("secondary body"),
        "second"
    );
    // The two documents are separate trees.
    assert_eq!(
        doc.eval("String(document.getElementById('here') !== null)")
            .expect("main lookup"),
        "true"
    );
    assert_eq!(
        doc.eval("String(secondary.getElementById('here') === null)")
            .expect("secondary lookup"),
        "true"
    );
    assert_eq!(
        doc.eval("String(secondary.documentElement.ownerDocument === secondary)")
            .expect("ownerDocument"),
        "true"
    );

    assert_eq!(
        doc.eval(
            "var xml = document.implementation.createDocument(null, '', null);\
             String(xml.createElement('x').namespaceURI)"
        )
        .expect("xml namespace"),
        "null"
    );
    assert_eq!(
        doc.eval(
            "var xhtml = document.implementation.createDocument(\
               'http://www.w3.org/1999/xhtml', 'html', null);\
             xhtml.contentType + '|' + String(xhtml.createElement('x').namespaceURI)"
        )
        .expect("xhtml document"),
        "application/xhtml+xml|http://www.w3.org/1999/xhtml"
    );
}

#[test]
fn mutation_observers_queue_and_deliver_records() {
    let (mut doc, _host) = document();
    doc.load_html("<!doctype html><body><div id=t>x</div></body>");
    doc.eval(
        "window.delivered = 0;\
         window.obs = new MutationObserver(function(records) { window.delivered += records.length; });\
         var t = document.getElementById('t');\
         obs.observe(t, {attributes: true, attributeOldValue: true, childList: true, subtree: true, characterData: true, characterDataOldValue: true});\
         t.setAttribute('x', '1');",
    )
    .expect("observe and mutate");
    // The queued delivery microtask runs with the eval's job drain.
    assert_eq!(doc.eval("String(window.delivered)").expect("delivery"), "1");
    // takeRecords drains the synchronous queue with old values.
    assert_eq!(
        doc.eval(
            "t.setAttribute('x', '2');\
             t.appendChild(document.createTextNode('hi'));\
             t.lastChild.data = 'bye';\
             var taken = obs.takeRecords();\
             taken.map(function(r) { return r.type + ':' + String(r.oldValue); }).join(',')"
        )
        .expect("takeRecords"),
        "attributes:1,childList:null,characterData:hi"
    );
    // Records already delivered are not re-delivered.
    assert_eq!(doc.eval("String(window.delivered)").expect("delivery"), "1");
}
