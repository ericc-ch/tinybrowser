//! `window.open`'s browser-tab path: a cross-origin popup stays a tab, and the
//! tab's first request runs under the calling document rather than the blank
//! document the tab starts with. The referrer comes from the opener's policy
//! and `Sec-Fetch-Site` from the opener's origin
//! (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps>).

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
/// `cross-site` rather than the blank `none` the new tab used to send.
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
