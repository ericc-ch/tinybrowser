//! `crossorigin` at the request boundary: the attribute decides CORS mode and
//! the credentials the request carries, and its invalid value default must not
//! send credentials.

use std::time::{Duration, Instant};

use crate::fixture::{PIXEL, RecordedHeads, ok, open_page, recording_server, serve_on, server};

/// Waits for the request whose target is `path` and returns its head,
/// lowercased so header-name casing on the wire does not matter.
fn head_for(heads: &RecordedHeads, path: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let recorded = heads.lock().expect("request log");
        if let Some(head) = recorded.iter().find(|head| {
            head.to_ascii_lowercase()
                .starts_with(&format!("get {path} "))
        }) {
            return head.to_ascii_lowercase();
        }
        drop(recorded);
        assert!(
            Instant::now() < deadline,
            "no request for {path}: {:?}",
            heads.lock().expect("request log")
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A padded `crossorigin` names no keyword, so it takes the invalid value
/// default: CORS mode with `same-origin` credentials. The request must be a
/// CORS request and must not send the cookie the page set, while the
/// `use-credentials` spelling on the same origin does
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
#[test]
fn a_padded_crossorigin_image_is_a_cors_request_without_credentials() {
    let (foreign, heads) = recording_server("image/png", PIXEL);
    let page = format!(
        r#"<!doctype html><body>
<script>document.cookie = 'session=1; path=/'</script>
<img src="http://{foreign}/padded.png" crossorigin=" use-credentials">
<img src="http://{foreign}/credentials.png" crossorigin="use-credentials">
"#
    );
    let (listener, origin) = server();
    serve_on(listener, vec![("/", ok("text/html", page.as_bytes()))]);
    let (_fixture, _client, _session) =
        open_page("tinybrowser-crossorigin", &format!("http://{origin}/"));

    let padded = head_for(&heads, "/padded.png");
    assert!(padded.contains("sec-fetch-mode: cors"), "{padded}");
    assert!(
        padded.contains(&format!("origin: http://{origin}")),
        "{padded}"
    );
    assert!(
        !padded.contains("cookie:"),
        "an invalid value must not send credentials: {padded}"
    );

    let credentialed = head_for(&heads, "/credentials.png");
    assert!(
        credentialed.contains("sec-fetch-mode: cors"),
        "{credentialed}"
    );
    assert!(
        credentialed.contains("cookie: session=1"),
        "use-credentials admits the cookie: {credentialed}"
    );
}
