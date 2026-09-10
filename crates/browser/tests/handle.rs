use std::time::{Duration, SystemTime, UNIX_EPOCH};

use browser::{Browser, PageEvent, Profile, RemoteValue, Renderers};

fn temp_data_home() -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("tinybrowser-handle-{stamp}"));
    std::fs::create_dir_all(&dir).expect("temp data home");
    dir
}

#[test]
fn page_handle_commands_are_values_only() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    assert_eq!(browser.profile_name().as_str(), "default");
    let handle = browser.handle();
    let page = handle.create_page().expect("page");
    let first = page.next_request_id();
    page.load_html("<!doctype html><p id=x>hi</p>")
        .expect("load html");
    assert_ne!(page.next_request_id().get(), first.get());
    assert_eq!(
        page.eval("document.getElementsByTagName('p')[0].firstChild.data")
            .expect("text"),
        "hi"
    );
    assert_eq!(
        page.execute_script("1 + 1").expect("number"),
        RemoteValue::Number(2.0)
    );
    assert_eq!(
        page.execute_script("null").expect("null"),
        RemoteValue::Null
    );
    assert_eq!(
        page.execute_script("undefined").expect("undefined"),
        RemoteValue::Undefined
    );
    assert!(matches!(
        page.execute_script("document.body").expect("node"),
        RemoteValue::Node(_)
    ));
    page.set_document_url("http://example.test/")
        .expect("document url");
    page.set_document_cookie("a=1").expect("cookie");
    assert_eq!(page.document_cookie().expect("cookie get"), "a=1");
    assert!(!page.last_navigation_failed().expect("nav"));
    assert_eq!(page.events().expect("events"), vec![PageEvent::Load]);
    page.shutdown().expect("shutdown");
    handle.close_page(page.id()).expect("remove stopped page");
    handle
        .close_page(page.id())
        .expect_err("unknown after close");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn execute_script_reuses_one_remote_id_for_the_same_node() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.load_html("<!doctype html><p>hi</p>").expect("load");
    let value = page
        .execute_script("[document.body, document.body]")
        .expect("pair");
    let RemoteValue::List(items) = value else {
        panic!("expected list, got {value:?}");
    };
    assert_eq!(items.len(), 2, "{items:?}");
    match (&items[0], &items[1]) {
        (RemoteValue::Node(first), RemoteValue::Node(second)) => {
            assert_eq!(first, second, "same node must intern to one remote id");
        }
        other => panic!("expected node pair, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn page_actor_advances_timers_without_a_run_command() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.load_html("<!doctype html><title></title>")
        .expect("load");
    page.eval("globalThis.fired = false; setTimeout(() => { fired = true; }, 10)")
        .expect("timer");

    std::thread::sleep(Duration::from_millis(50));

    assert_eq!(page.eval("String(fired)").expect("fired"), "true");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn page_events_are_pushed_to_subscribers() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    let events = page.subscribe().expect("subscribe");

    page.load_html("<!doctype html><title></title>")
        .expect("load");

    assert_eq!(
        events.recv_timeout(Duration::from_secs(1)).expect("event"),
        PageEvent::Load
    );
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn a_wait_does_not_monopolize_the_page_actor() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.load_html("<!doctype html><title></title>")
        .expect("load");
    page.eval("setTimeout(() => {}, 250)").expect("timer");
    let first_request = page.next_request_id().get();
    let waiter = page.clone();
    let waiting = std::thread::spawn(move || waiter.run());
    let submitted = std::time::Instant::now() + Duration::from_secs(1);
    while page.next_request_id().get() == first_request {
        assert!(
            std::time::Instant::now() < submitted,
            "wait was not submitted"
        );
        std::thread::yield_now();
    }

    let started = std::time::Instant::now();
    assert_eq!(page.eval("String(1 + 1)").expect("concurrent eval"), "2");
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "wait command monopolized the actor"
    );
    waiting.join().expect("wait thread").expect("wait result");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn dom_mutation_survives_a_failed_parser_blocking_script() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    let (script_requested, requested) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut page, _) = listener.accept().expect("page request");
        let mut request = [0_u8; 512];
        let _bytes_read = page.read(&mut request).expect("read page request");
        let body = b"<!doctype html><body><div id=before></div><script src=/missing></script><p id=after></p>";
        write!(
            page,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("page head");
        page.write_all(body).expect("page body");
        drop(page);

        let (mut script, _) = listener.accept().expect("script request");
        let _bytes_read = script.read(&mut request).expect("read script request");
        script_requested.send(()).expect("requested signal");
        released.recv().expect("release script");
    });

    let browser = Browser::ephemeral().expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.goto(&format!("http://{addr}/")).expect("goto");
    requested
        .recv_timeout(Duration::from_secs(1))
        .expect("external script request");
    page.eval("document.getElementById('before').id = 'mutated'")
        .expect("mutation while parser paused");
    release.send(()).expect("release");
    assert!(
        page.run_until_load_timeout(Duration::from_secs(1))
            .expect("load wait")
    );
    assert_eq!(
        page.eval(
            "document.getElementById('mutated').id + ':' + document.getElementById('after').id"
        )
        .expect("completed document"),
        "mutated:after"
    );
    server.join().expect("server");
}

#[test]
fn cross_site_navigation_swaps_the_document() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn serve(listener: &TcpListener, body: &'static [u8]) {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut chunk = [0_u8; 1024];
        let _ = stream.read(&mut chunk).expect("request");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("head");
        stream.write_all(body).expect("body");
    }

    let first = TcpListener::bind("127.0.0.1:0").expect("first bind");
    let first_addr = first.local_addr().expect("first addr");
    let second = TcpListener::bind("127.0.0.1:0").expect("second bind");
    let second_addr = second.local_addr().expect("second addr");
    let first_server =
        std::thread::spawn(move || serve(&first, b"<!doctype html><title>a</title>"));
    let second_server =
        std::thread::spawn(move || serve(&second, b"<!doctype html><title>b</title>"));

    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.goto(&format!("http://{first_addr}/")).expect("goto a");
    page.run_until_load().expect("load a");
    page.eval("globalThis.secret = 'a'").expect("secret");

    // `localhost` and `127.0.0.1` are different sites: the tab must mount the
    // new document in a renderer for the new site, not reuse the old realm.
    page.goto(&format!("http://localhost:{}/", second_addr.port()))
        .expect("goto b");
    page.run_until_load().expect("load b");
    assert_eq!(
        page.document_url().expect("url"),
        format!("http://localhost:{}/", second_addr.port())
    );
    assert_eq!(
        page.eval("typeof globalThis.secret").expect("realm"),
        "undefined"
    );

    first_server.join().expect("first server");
    second_server.join().expect("second server");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn failed_navigation_is_reported() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.goto("http://127.0.0.1:1/").expect("queued");
    page.run_until_load().expect("load wait");
    assert!(page.last_navigation_failed().expect("nav"));
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn shutdown_interrupts_a_running_script() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.load_html("<!doctype html><title></title>")
        .expect("load");
    let first_request = page.next_request_id().get();
    let evaluator = page.clone();
    let running = std::thread::spawn(move || evaluator.eval("while (true) {}"));
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while page.next_request_id().get() == first_request {
        assert!(
            std::time::Instant::now() < deadline,
            "eval was not submitted"
        );
        std::thread::yield_now();
    }

    let started = std::time::Instant::now();
    page.shutdown().expect("shutdown");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown did not interrupt script"
    );
    assert!(running.join().expect("evaluator").is_err());
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn webidl_node_name_doctype_and_branding() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.load_html("<!doctype html><title></title>")
        .expect("load");
    let htmlns = "http://www.w3.org/1999/xhtml";
    let svgns = "http://www.w3.org/2000/svg";
    let got = page
        .eval(&format!(
            r#"
            [
              document.createElementNS("{htmlns}", "I").nodeName,
              document.createElementNS("{htmlns}", "i").nodeName,
              document.createElementNS("{svgns}", "svg").nodeName,
              document.createElementNS("{svgns}", "SVG").nodeName,
              document.createElementNS("{htmlns}", "x:b").nodeName,
              document.createTextNode("foo").nodeName,
              document.createComment("foo").nodeName,
              document.nodeName,
              document.doctype.nodeName,
              document.createDocumentFragment().nodeName,
              typeof EventTarget,
              typeof Document,
              String(Object.getPrototypeOf(Node.prototype) === EventTarget.prototype),
              String(Object.getPrototypeOf(document) === Document.prototype),
              String(document instanceof Document),
              String(document instanceof Node),
              String(document instanceof EventTarget),
              String(document.createElement("p") instanceof Element),
              String(document.createTextNode("x") instanceof Text),
              String(document.createComment("x") instanceof Comment),
              String(document.doctype instanceof DocumentType),
              String(document.createDocumentFragment() instanceof DocumentFragment),
              String(document.body === document.body),
              String(document.doctype === document.doctype),
              String(document.createElement("p") instanceof Node),
              String(document.createTextNode("x") instanceof CharacterData),
              typeof document.createTextNode("x").createElement,
              typeof document.createTextNode("x").getAttribute,
              typeof document.createElement("p").createElement,
              typeof document.createElement("p").data,
              typeof document.getAttribute,
              (function() {{
                var node = document.createComment("x");
                return String(document.documentElement.appendChild(node) === node);
              }})(),
              (function() {{
                try {{ document.createElementNS(null, "a:b"); return "no"; }}
                catch (e) {{ return String(e).indexOf("NamespaceError") >= 0; }}
              }})(),
              (function() {{
                try {{ document.createElementNS("http://example.test", "xml:x"); return "no"; }}
                catch (e) {{ return String(e).indexOf("NamespaceError") >= 0; }}
              }})(),
              (function() {{
                try {{ document.createElementNS("{htmlns}", "a:b:c"); return "no"; }}
                catch (e) {{ return String(e).indexOf("InvalidCharacterError") >= 0; }}
              }})(),
              (function() {{
                try {{ document.createElementNS("{htmlns}", ":a"); return "no"; }}
                catch (e) {{ return String(e).indexOf("InvalidCharacterError") >= 0; }}
              }})(),
              (function() {{
                try {{ document.createElementNS("{htmlns}", "a:"); return "no"; }}
                catch (e) {{ return String(e).indexOf("InvalidCharacterError") >= 0; }}
              }})(),
              (function() {{
                try {{ document.createElementNS("{htmlns}", "a b"); return "no"; }}
                catch (e) {{ return String(e).indexOf("InvalidCharacterError") >= 0; }}
              }})(),
              (function() {{
                try {{ document.createElementNS("{htmlns}", "a!"); return "no"; }}
                catch (e) {{ return String(e).indexOf("InvalidCharacterError") >= 0; }}
              }})(),
              (function() {{
                Element = 1;
                return document.createElement("p").nodeName;
              }})()
            ].join("|")
            "#
        ))
        .expect("webidl");
    assert_eq!(
        got,
        "I|I|svg|SVG|X:B|#text|#comment|#document|html|#document-fragment|function|function|true|true|true|true|true|true|true|true|true|true|true|true|true|true|undefined|undefined|undefined|undefined|undefined|true|true|true|true|true|true|true|true|P"
    );
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn dom_collections_are_live_host_objects() {
    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.load_html("<!doctype html><body></body>")
        .expect("load");

    let result = page
        .eval(
            r"
var elements = document.getElementsByTagName('p');
var children = document.body.childNodes;
var before = elements.length + ':' + children.length;
var paragraph = document.createElement('p');
paragraph.id = 'later';
document.body.appendChild(paragraph);
[
  before,
  elements.length,
  elements.item(0).id,
  elements[0].id,
  children.length,
  children[0].id,
  elements instanceof HTMLCollection,
  children instanceof NodeList
].join('|')
",
        )
        .expect("collections");

    assert_eq!(result, "0:0|1|later|later|1|later|true|true");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn page_handle_run_until_load_does_not_wait_for_unrelated_fetch() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Instant;

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

    fn respond(stream: &mut TcpStream, body: &[u8]) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/html\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("head");
        stream.write_all(body).expect("body");
    }

    let slow = TcpListener::bind("127.0.0.1:0").expect("slow bind");
    let slow_addr = slow.local_addr().expect("slow addr");
    let page_listener = TcpListener::bind("127.0.0.1:0").expect("page bind");
    let page_addr = page_listener.local_addr().expect("page addr");
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
        respond(&mut stream, b"slow");
    });
    let page_server = thread::spawn(move || {
        let (mut stream, _) = page_listener.accept().expect("page accept");
        let _ = read_target(&mut stream);
        let html = format!(
            "<!doctype html><script>fetch('http://{slow_addr}/slow').then(function() {{ window.slowDone = true; }});</script>"
        );
        respond(&mut stream, html.as_bytes());
    });

    let data_home = temp_data_home();
    let browser =
        Browser::open_in_with(&data_home, &Profile::default(), Renderers::Local).expect("browser");
    let page = browser.handle().create_page().expect("page");
    page.goto(&format!("http://{page_addr}/")).expect("goto");
    let started = Instant::now();
    page.run_until_load().expect("load");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "handle load waited for unrelated fetch: {:?}",
        started.elapsed()
    );
    assert!(!page.last_navigation_failed().expect("nav"));
    assert_eq!(
        page.eval("typeof window.slowDone").expect("slow"),
        "undefined"
    );
    release.store(true, Ordering::SeqCst);
    page.run().expect("drain");
    assert_eq!(page.eval("String(window.slowDone)").expect("done"), "true");
    slow_server.join().expect("slow server");
    page_server.join().expect("page server");
    let _ = std::fs::remove_dir_all(data_home);
}
