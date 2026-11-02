//! The `isSecureContext` bit of a document and the `[SecureContext]` surfaces
//! it gates, across a real frame tree rather than a hand-built one.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::fixture::{TestServer, evaluate, ok, open_page, serve_on, server};

/// Percent-encodes `markup` into a `data:` URL.
fn data_url(markup: &str) -> String {
    let mut url = String::from("data:text/html,");
    for byte in markup.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                url.push(char::from(byte));
            }
            _ => {
                let _ = write!(url, "%{byte:02X}");
            }
        }
    }
    url
}

/// A document whose only content is one `srcdoc` child.
const SRCDOC_PAGE: &str = r#"<!doctype html><body>
<iframe srcdoc="<script>
parent.__child = [
  isSecureContext,
  'userAgentData' in navigator,
  typeof NavigatorUAData,
  typeof crypto.randomUUID,
];
</script>"></iframe>
"#;

/// What the top-level document reports about itself.
const SELF_REPORT: &str = "[
  String(isSecureContext),
  String('userAgentData' in navigator),
  typeof NavigatorUAData,
  typeof crypto.randomUUID,
].join(',')";

/// Polls `expression` until it returns `expected`.
fn wait_for(client: &mut cdp::Client, session: &str, expression: &str, expected: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let value = evaluate(client, session, expression);
        if &value == expected {
            return value;
        }
        assert!(Instant::now() < deadline, "{expression}: {value}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A `data:` document is never a secure context, and a child of it inherits
/// that, however secure the child's own URL looks
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#secure-contexts>).
#[test]
fn a_data_document_is_never_secure_and_its_child_inherits_it() {
    let (_fixture, mut client, session) =
        open_page("tinybrowser-secure-context-data", &data_url(SRCDOC_PAGE));

    assert_eq!(
        evaluate(&mut client, &session, SELF_REPORT),
        json!("false,false,undefined,undefined"),
        "a data: top-level document is not a secure context"
    );
    wait_for(
        &mut client,
        &session,
        "window.__child ?? 'pending'",
        &json!([false, false, "undefined", "undefined"]),
    );
}

/// A child of a potentially trustworthy document inherits the bit, and an
/// `https` child of an insecure document stays insecure
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#secure-contexts>).
#[test]
fn a_child_inherits_its_creators_secure_context() {
    let (listener, page) = server();
    let body = format!("<!doctype html><body>{SRCDOC_PAGE}");
    serve_on(listener, vec![("/", ok("text/html", body.as_bytes()))]);
    let (_fixture, mut client, session) = open_page(
        "tinybrowser-secure-context-child",
        &format!("http://{page}/"),
    );

    // A loopback host is potentially trustworthy, so the parent and the
    // `srcdoc` child it creates are both secure contexts.
    assert_eq!(
        evaluate(&mut client, &session, SELF_REPORT),
        json!("true,true,function,function")
    );
    // Web IDL names the attribute getter `get userAgentData`
    // (<https://webidl.spec.whatwg.org/#idl-attributes>).
    assert_eq!(
        evaluate(
            &mut client,
            &session,
            "Object.getOwnPropertyDescriptor(Navigator.prototype, 'userAgentData').get.name"
        ),
        json!("get userAgentData")
    );
    wait_for(
        &mut client,
        &session,
        "window.__child ?? 'pending'",
        &json!([true, true, "function", "function"]),
    );
}

/// A popup's creator is its opener, so it inherits the opener's secure-context
/// bit the same way a local-scheme frame does. An `about:blank` or `blob:`
/// popup opened by a `data:` document is insecure
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#secure-contexts>).
#[test]
fn a_popup_inherits_its_openers_insecure_context() {
    // A `data:` document's blob is opaque-origin and the blob opens no
    // opener, so the popup reports its own bit through an image request.
    let markers = TestServer::start(vec![], None);
    let marker_port = markers.address().port();
    let page = format!(
        r#"<!doctype html><script>
const blank = open('about:blank');
const blobMarkup = '<body onload="new Image().src = \'http://127.0.0.1:{marker_port}/\' + isSecureContext">';
const blob = open(URL.createObjectURL(new Blob([blobMarkup], {{type: 'text/html'}})));
window.__blankBit = () => {{
  try {{ return String(blank.isSecureContext); }}
  catch (error) {{ return 'pending'; }}
}};
</script>"#
    );
    let (_fixture, mut client, session) =
        open_page("tinybrowser-secure-context-popup", &data_url(&page));

    assert_eq!(
        evaluate(&mut client, &session, "String(isSecureContext)"),
        json!("false")
    );
    wait_for(
        &mut client,
        &session,
        "window.__blankBit()",
        &json!("false"),
    );
    markers.wait("/false");
}

/// The mirror case: a loopback document is secure, and the blank and blob
/// popups it opens are secure creators' children
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#secure-contexts>).
#[test]
fn a_popup_inherits_its_openers_secure_context() {
    let page = r#"<!doctype html><script>
const blank = open('about:blank');
const blobMarkup = '<scr' + 'ipt>opener.postMessage(String(isSecureContext), "*")</scr' + 'ipt>';
const blob = open(URL.createObjectURL(new Blob([blobMarkup], {type: 'text/html'})));
window.__blobBit = null;
addEventListener('message', (event) => { window.__blobBit = event.data; });
window.__blankBit = () => {
  try { return String(blank.isSecureContext); } catch (error) { return 'pending'; }
};
</script>"#;
    let (listener, address) = server();
    serve_on(listener, vec![("/", ok("text/html", page.as_bytes()))]);
    let (_fixture, mut client, session) = open_page(
        "tinybrowser-secure-context-popup-secure",
        &format!("http://127.0.0.1:{}/", address.port()),
    );

    wait_for(&mut client, &session, "window.__blankBit()", &json!("true"));
    wait_for(&mut client, &session, "window.__blobBit", &json!("true"));
}
