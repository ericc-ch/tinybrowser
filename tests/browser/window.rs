//! `window.open`'s browser-tab path: a cross-origin popup stays a tab, and the
//! tab's first request runs under the calling document rather than the blank
//! document the tab starts with. The referrer comes from the opener's policy
//! and `Sec-Fetch-Site` from the opener's origin
//! (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>).

use std::time::{Duration, Instant};

use crate::fixture::{Fixture, TestServer, evaluate, ok, open_page, serve_on, server};

/// Serves one page that opens a browser-hosted popup, and returns the popup
/// server plus the page's serialized origin.
///
/// The page binds the loopback address; `page_host` decides what site it
/// claims, so `localhost` and `127.0.0.1` let a test choose same-site or
/// cross-site against the popup, which is always `localhost`.
fn open_popup(
    page_host: &str,
    head: &str,
    features: &str,
) -> (TestServer, String, Fixture, cdp::Client, String) {
    let popup = TestServer::start(vec![("/", "text/html", b"<!doctype html>peer")], None);
    let target = format!("http://localhost:{}/", popup.address().port());
    let (listener, address) = server();
    let page = format!(
        r#"<!doctype html><html><head>{head}</head><body>
<script>window.__popup = window.open({target:?}, "peer", {features:?});</script>
"#
    );
    serve_on(listener, vec![("/start", ok("text/html", page.as_bytes()))]);
    let page_origin = format!("http://{page_host}:{}", address.port());
    let (fixture, client, session) =
        open_page("tinybrowser-window-open", &format!("{page_origin}/start"));
    (popup, page_origin, fixture, client, session)
}

/// The default policy is `strict-origin-when-cross-origin`, so a cross-site
/// popup gets the opener's origin and not its path, and the navigation is
/// `cross-site`.
#[test]
fn a_cross_site_popup_sends_the_openers_origin_as_referrer() {
    let (popup, origin, _fixture, mut client, session) = open_popup("127.0.0.1", "", "");
    let head = popup.wait("/").to_ascii_lowercase();
    assert!(
        head.contains(&format!("referer: {origin}/")),
        "the popup carries the opener's origin: {head}"
    );
    assert!(head.contains("sec-fetch-site: cross-site"), "{head}");
    assert_eq!(
        evaluate(&mut client, &session, "window.__popup !== null"),
        serde_json::json!(true),
        "with `noopener` absent the call returns a `WindowProxy`"
    );
}

/// The calling document's own policy decides how much of its URL travels. A
/// page that opts into `unsafe-url` sends the whole path, which proves the
/// referrer is resolved from the opener's policy container and not a constant.
#[test]
fn the_openers_referrer_policy_decides_how_much_travels() {
    let (popup, origin, _fixture, _client, _session) = open_popup(
        "127.0.0.1",
        r#"<meta name="referrer" content="unsafe-url">"#,
        "",
    );
    let head = popup.wait("/").to_ascii_lowercase();
    assert!(
        head.contains(&format!("referer: {origin}/start")),
        "`unsafe-url` sends the full opener URL: {head}"
    );
    assert!(head.contains("sec-fetch-site: cross-site"), "{head}");
}

/// `noreferrer` is the opener feature's referrer half: the popup still opens
/// and still runs under the calling document's origin, but sends no `Referer`.
#[test]
fn noreferrer_opens_the_popup_without_a_referrer() {
    let (popup, _origin, _fixture, _client, _session) = open_popup("127.0.0.1", "", "noreferrer");
    let head = popup.wait("/").to_ascii_lowercase();
    assert!(
        !head.contains("referer:"),
        "`noreferrer` withholds the header: {head}"
    );
    assert!(head.contains("sec-fetch-site: cross-site"), "{head}");
}

/// A popup from a different origin on the same site is still a browser tab,
/// and its navigation is `same-site` while the default policy sends the
/// origin.
#[test]
fn a_same_site_popup_is_same_site_and_not_same_origin() {
    let (popup, origin, _fixture, _client, _session) = open_popup("localhost", "", "");
    let head = popup.wait("/").to_ascii_lowercase();
    assert!(
        head.contains(&format!("referer: {origin}/")),
        "the popup carries the opener's origin: {head}"
    );
    assert!(head.contains("sec-fetch-site: same-site"), "{head}");
}

/// A named browser-tab popup is one browsing context: opening the same name
/// again selects the tab the first call opened and navigates it, instead of
/// creating a second tab
/// (<https://html.spec.whatwg.org/multipage/document-sequences.html#the-rules-for-choosing-a-navigable>,
/// <https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>).
#[test]
fn a_named_popup_is_reused_and_navigated() {
    let popup = TestServer::start(
        vec![
            ("/first", "text/html", b"<!doctype html>first"),
            ("/second", "text/html", b"<!doctype html>second"),
        ],
        None,
    );
    let port = popup.address().port();
    let (listener, address) = server();
    // The page is `127.0.0.1`, so the popups are cross-site and stay browser
    // tabs; the browser has to own the name table for them.
    let page = format!(
        r#"<!doctype html><script>
window.__first = window.open('http://localhost:{port}/first', "shared");
</script>"#
    );
    serve_on(listener, vec![("/start", ok("text/html", page.as_bytes()))]);
    let (_fixture, mut client, session) = open_page(
        "tinybrowser-window-open",
        &format!("http://127.0.0.1:{}/start", address.port()),
    );
    popup.wait_count("/first", 1);
    // The second open happens after the first tab has loaded, so its
    // navigation is not superseded by the first one.
    let second = format!(
        r#"window.__second = window.open('http://localhost:{port}/second', "shared");
           window.__first === window.__second"#
    );
    assert_eq!(
        evaluate(&mut client, &session, &second),
        serde_json::json!(true),
        "the second call with the same name selects the first tab"
    );
    popup.wait_count("/second", 1);
    assert_eq!(
        popup.history("/first").len(),
        1,
        "the reused name did not open a second tab"
    );
    assert_eq!(
        popup.history("/second").len(),
        1,
        "the selected tab navigated to the second URL"
    );
}

/// A window open with no URL navigates nothing when it selects an existing
/// context: `window.open('', '_self')` is how a page asks for its own proxy,
/// and it must not replace the document with `about:blank`
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>).
#[test]
fn opening_the_current_context_with_no_url_keeps_its_document() {
    let (listener, address) = server();
    let page = "<!doctype html><title>stay</title><script>\
        window.__marker = 1;\
        window.__self = window.open('', '_self');\
        window.__top = window.open('', '_top');\
        </script>";
    serve_on(listener, vec![("/start", ok("text/html", page.as_bytes()))]);
    let (_fixture, mut client, session) = open_page(
        "tinybrowser-window-open",
        &format!("http://127.0.0.1:{}/start", address.port()),
    );
    assert_eq!(
        evaluate(
            &mut client,
            &session,
            "[window.__self === window, window.__top === window, window.__marker, document.title].join(',')"
        ),
        serde_json::json!("true,true,1,stay")
    );
}

/// A sandboxed document without `allow-popups` cannot create a context, no
/// matter how the target is spelled. The old check tested for the literal
/// `_blank`, so `_BLANK` and any unknown `_`-prefixed name walked past it and
/// opened a cross-origin tab. `_self` keeps working, because it creates
/// nothing
/// (<https://html.spec.whatwg.org/multipage/browsers.html#sandboxed-auxiliary-navigation-browsing-context-flag>).
#[test]
fn a_sandbox_without_allow_popups_cannot_open_a_context_by_spelling() {
    let popup = TestServer::start(
        vec![
            ("/allowed", "text/html", b"<!doctype html>allowed"),
            ("/blank", "text/html", b"<!doctype html>blank"),
            ("/upper", "text/html", b"<!doctype html>upper"),
            ("/named", "text/html", b"<!doctype html>named"),
        ],
        None,
    );
    let port = popup.address().port();
    let frame = |sandbox: &str, script: &str| {
        format!("<iframe sandbox=\"{sandbox}\" srcdoc=\"<script>{script}</script>\"></iframe>")
    };
    // A sandboxed `srcdoc` frame is opaque-origin, so it cannot write the
    // parent's globals; `postMessage` is the one channel it has.
    let page = format!(
        "<!doctype html><script>\
         window.__answers = [];\
         window.addEventListener('message', function (event) {{\
           window.__answers.push(String(event.data));\
         }});\
         </script>{}{}{}",
        frame(
            "allow-scripts allow-popups",
            &format!(
                "parent.postMessage('allowed:' + (window.open('http://localhost:{port}/allowed', '_blank') !== null), '*')"
            ),
        ),
        frame(
            "allow-scripts",
            &format!(
                "parent.postMessage(['refused', \
                 window.open('http://localhost:{port}/blank', '_blank') === null, \
                 window.open('http://localhost:{port}/upper', '_BLANK') === null, \
                 window.open('http://localhost:{port}/named', '_new') === null].join(','), '*')"
            ),
        ),
        frame(
            "allow-scripts",
            "parent.postMessage('self:' + (window.open('', '_self') === window), '*')",
        ),
    );
    let (listener, address) = server();
    serve_on(listener, vec![("/start", ok("text/html", page.as_bytes()))]);
    let (_fixture, mut client, session) = open_page(
        "tinybrowser-window-open",
        &format!("http://127.0.0.1:{}/start", address.port()),
    );

    // The synchronous answers: the permitted frame got a proxy, the sandboxed
    // frames got null for every spelling of a new context, and `_self` still
    // selects its own window.
    wait_for_answers(&mut client, &session);
    assert_eq!(
        evaluate(
            &mut client,
            &session,
            "window.__answers.slice().sort().join('|')"
        ),
        serde_json::json!("allowed:true|refused,true,true,true|self:true")
    );
    // The permitted frame's tab really navigated; the refused names issued no
    // request. A refused call would have started its request in the same task
    // as the permitted one, so its absence after the permitted request lands is
    // meaningful.
    popup.wait("/allowed");
    std::thread::sleep(std::time::Duration::from_millis(300));
    for path in ["/blank", "/upper", "/named"] {
        assert!(!popup.requested(path), "{path} opened without allow-popups");
    }
}

/// Waits until each sandboxed frame has posted its one answer.
fn wait_for_answers(client: &mut cdp::Client, session: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if evaluate(client, session, "window.__answers.length") == serde_json::json!(3) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the sandboxed frames never reported"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
