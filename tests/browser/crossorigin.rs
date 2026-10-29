//! `crossorigin` at the request boundary: the attribute decides CORS mode and
//! the credentials the request carries, and its invalid value default must not
//! send credentials.

use crate::fixture::{PIXEL, TestServer, ok, open_page, serve_on, server};

/// A padded `crossorigin` names no keyword, so it takes the invalid value
/// default: CORS mode with `same-origin` credentials. The request must be a
/// CORS request and must not send the cookie the page set, while the
/// `use-credentials` spelling on the same origin does
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#cors-settings-attributes>).
#[test]
fn a_padded_crossorigin_image_is_a_cors_request_without_credentials() {
    let foreign = TestServer::start(
        vec![
            ("/padded.png", "image/png", PIXEL),
            ("/credentials.png", "image/png", PIXEL),
        ],
        None,
    );
    let page = format!(
        r#"<!doctype html><body>
<script>document.cookie = 'session=1; path=/'</script>
<img src="http://{}/padded.png" crossorigin=" use-credentials">
<img src="http://{}/credentials.png" crossorigin="use-credentials">
"#,
        foreign.address(),
        foreign.address()
    );
    let (listener, origin) = server();
    serve_on(listener, vec![("/", ok("text/html", page.as_bytes()))]);
    let (_fixture, _client, _session) =
        open_page("tinybrowser-crossorigin", &format!("http://{origin}/"));

    let padded = foreign.wait("/padded.png").to_ascii_lowercase();
    assert!(padded.contains("sec-fetch-mode: cors"), "{padded}");
    assert!(
        padded.contains(&format!("origin: http://{origin}")),
        "{padded}"
    );
    assert!(
        !padded.contains("cookie:"),
        "an invalid value must not send credentials: {padded}"
    );

    let credentialed = foreign.wait("/credentials.png").to_ascii_lowercase();
    assert!(
        credentialed.contains("sec-fetch-mode: cors"),
        "{credentialed}"
    );
    assert!(
        credentialed.contains(&format!("origin: http://{origin}")),
        "{credentialed}"
    );
    assert!(
        credentialed.contains("cookie: session=1"),
        "use-credentials admits the cookie: {credentialed}"
    );
}

/// The gap behind A5: `crossorigin` is recovered by URL, so two images on the
/// same URL with different attributes share one request. The fork change that
/// puts the element on Blitz's request makes this pass.
#[test]
#[ignore = "A5: the request carries no element, so crossorigin is recovered by URL"]
fn two_images_on_one_url_keep_their_own_crossorigin() {
    let foreign = TestServer::start(vec![("/shared.png", "image/png", PIXEL)], None);
    let page = format!(
        r#"<!doctype html><body>
<script>document.cookie = 'session=1; path=/'</script>
<img src="http://{0}/shared.png" crossorigin="anonymous">
<img src="http://{0}/shared.png" crossorigin="use-credentials">
"#,
        foreign.address()
    );
    let (listener, origin) = server();
    serve_on(listener, vec![("/", ok("text/html", page.as_bytes()))]);
    let (_fixture, _client, _session) = open_page(
        "tinybrowser-crossorigin-shared",
        &format!("http://{origin}/"),
    );

    foreign.wait("/shared.png");
    let requests = foreign.history("/shared.png");
    assert_eq!(requests.len(), 2, "one request per image");
    assert!(
        requests
            .iter()
            .any(|head| !head.to_ascii_lowercase().contains("cookie:")),
        "the anonymous request must not send the cookie"
    );
    assert!(
        requests
            .iter()
            .any(|head| head.to_ascii_lowercase().contains("cookie: session=1")),
        "the credentialed request must send the cookie"
    );
}
