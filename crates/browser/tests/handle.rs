use std::time::{Duration, SystemTime, UNIX_EPOCH};

use browser::{Browser, PageEvent, Profile, RemoteValue};

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
    let browser = Browser::open_in(&data_home, &Profile::default());
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
    assert!(matches!(
        page.execute_script("document.body").expect("node"),
        RemoteValue::Node(_)
    ));
    page.set_document_url("http://example.test/")
        .expect("document url");
    page.set_document_cookie("a=1").expect("cookie");
    assert_eq!(page.document_cookie().expect("cookie get"), "a=1");
    assert!(!page.last_navigation_failed().expect("nav"));
    assert_eq!(page.events().expect("events"), Vec::<PageEvent>::new());
    page.shutdown().expect("shutdown");
    handle.close_page(page.id()).expect("remove stopped page");
    handle
        .close_page(page.id())
        .expect_err("unknown after close");
    let _ = std::fs::remove_dir_all(data_home);
}

#[test]
fn webidl_node_name_doctype_and_branding() {
    let data_home = temp_data_home();
    let browser = Browser::open_in(&data_home, &Profile::default());
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
              typeof Document,
              String(Object.getPrototypeOf(document) === Document.prototype),
              String(document instanceof Document),
              String(document instanceof Node),
              String(document.createElement("p") instanceof Element),
              String(document.createTextNode("x") instanceof Text),
              String(document.createComment("x") instanceof Comment),
              String(document.doctype instanceof DocumentType),
              String(document.createDocumentFragment() instanceof DocumentFragment),
              String(document.body === document.body),
              String(document.doctype === document.doctype),
              String(document.createElement("p") instanceof Node),
              (function() {{
                var node = document.createComment("x");
                return String(document.documentElement.appendChild(node) === node);
              }})()
            ].join("|")
            "#
        ))
        .expect("webidl");
    assert_eq!(
        got,
        "I|I|svg|SVG|X:B|#text|#comment|#document|html|#document-fragment|function|true|true|true|true|true|true|true|true|true|true|true|true"
    );
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
    let browser = Browser::open_in(&data_home, &Profile::default());
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
