//! Image request lifecycle: what the element reports for a broken source, and
//! whether the document's `load` event waits for image requests.

use serde_json::json;

use crate::fixture::{PIXEL, TestServer, evaluate, ok, open_page, poll_until, serve_on, server};

/// An empty `src` is a broken image request: the element fires `error` and the
/// document's `load` event does not wait for a request that can never settle
/// (<https://html.spec.whatwg.org/multipage/images.html#updating-the-image-data>).
#[test]
fn an_empty_src_fires_error_and_does_not_hold_load() {
    let page = r#"<!doctype html><body>
<img id="img" src="">
<script>
window.__events = [];
const image = document.getElementById('img');
image.addEventListener('error', () => window.__events.push('error'));
image.addEventListener('load', () => window.__events.push('load'));
</script>
"#;
    let (listener, origin) = server();
    serve_on(listener, vec![("/", ok("text/html", page.as_bytes()))]);
    // A document whose load event never fires fails this call.
    let (_fixture, mut client, session) =
        open_page("tinybrowser-image-empty-src", &format!("http://{origin}/"));
    assert_eq!(
        evaluate(&mut client, &session, "window.__events.join(',')"),
        json!("error")
    );
    assert_eq!(
        evaluate(
            &mut client,
            &session,
            "String(document.getElementById('img').complete)"
        ),
        json!("true")
    );
}

/// Moving an image in the tree keeps its request: insertion only restarts when
/// the element's request state changed
/// (<https://html.spec.whatwg.org/multipage/images.html#reacting-to-dom-mutations>).
#[test]
fn moving_a_loaded_image_does_not_reload_it() {
    let foreign = TestServer::start(vec![("/pixel.png", "image/png", PIXEL)], None);
    let page = format!(
        r#"<!doctype html><body>
<div id="first"></div><div id="second"></div>
<img id="img" src="http://{0}/pixel.png">
<script>
window.__loads = 0;
const image = document.getElementById('img');
image.addEventListener('load', () => window.__loads++);
</script>
"#,
        foreign.address()
    );
    let (listener, origin) = server();
    serve_on(listener, vec![("/", ok("text/html", page.as_bytes()))]);
    let (_fixture, mut client, session) =
        open_page("tinybrowser-image-move", &format!("http://{origin}/"));
    poll_until("the first load", || {
        (evaluate(&mut client, &session, "window.__loads") == json!(1)).then_some(())
    });

    evaluate(
        &mut client,
        &session,
        "document.getElementById('second').append(document.getElementById('img'))",
    );
    // A reload would be visible as `complete` turning false on the next turn.
    assert_eq!(
        evaluate(
            &mut client,
            &session,
            "String(document.getElementById('img').complete)"
        ),
        json!("true")
    );
    assert_eq!(evaluate(&mut client, &session, "window.__loads"), json!(1));
    assert_eq!(
        foreign.history("/pixel.png").len(),
        1,
        "a move must not dial again"
    );
}
