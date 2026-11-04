//! One tab's joint session history across frames: a child reports the tab's
//! step count, and the parent's `back()` traverses an entry the child made
//! (<https://html.spec.whatwg.org/multipage/history.html#joint-session-history>,
//! <https://html.spec.whatwg.org/multipage/browsing-the-web.html#traverse-the-history>).

use std::time::{Duration, Instant};

use crate::fixture::{TestServer, evaluate, ok, open_page};

/// Polls `expression` in `session` until it returns `expected`.
fn wait_for_value(
    client: &mut cdp::Client,
    session: &str,
    expression: &str,
    expected: &serde_json::Value,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let value = evaluate(client, session, expression);
        if value == *expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{expression} never became {expected}: last value {value}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Serves a page whose iframe starts on `/child1` and can be moved to
/// `/child2`.
fn joint_history_page() -> (TestServer, cdp::Client, String, crate::fixture::Fixture) {
    let page = b"<!doctype html><body><iframe id=f src=/child1></iframe>";
    let child = |name: &'static str| {
        format!("<!doctype html><body>{name}<script>window.__marker = {name:?};</script>")
    };
    let server = TestServer::start(
        vec![
            ("/", ok("text/html", page)),
            ("/child1", ok("text/html", child("one").as_bytes())),
            ("/child2", ok("text/html", child("two").as_bytes())),
        ],
        None,
    );
    let origin = format!("http://{}", server.address());
    let (fixture, client, session) = open_page("tinybrowser-joint-history", &format!("{origin}/"));
    (server, client, session, fixture)
}

#[test]
fn a_failed_child_navigation_does_not_add_a_joint_step() {
    let (_server, mut client, session, _fixture) = joint_history_page();
    let frame = "document.getElementById('f')";
    wait_for_value(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.__marker"),
        &serde_json::json!("one"),
    );
    assert_eq!(evaluate(&mut client, &session, "history.length"), 2);

    // Port 9 on loopback refuses the connection, so the frame keeps its
    // document. A navigation that never commits must not add a step
    // (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#finalize-a-cross-document-navigation>).
    evaluate(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.location.href = 'http://127.0.0.1:9/never'"),
    );
    std::thread::sleep(Duration::from_millis(300));
    wait_for_value(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.__marker"),
        &serde_json::json!("one"),
    );
    assert_eq!(
        evaluate(&mut client, &session, "history.length"),
        2,
        "a failed child load leaves the joint history alone"
    );

    // The frame still navigates after a failure, and that one does add a step.
    evaluate(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.location.href = '/child2'"),
    );
    wait_for_value(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.__marker"),
        &serde_json::json!("two"),
    );
    assert_eq!(evaluate(&mut client, &session, "history.length"), 3);
}

#[test]
fn a_child_reports_the_joint_step_count_and_the_parent_backs_into_it() {
    let (_server, mut client, session, _fixture) = joint_history_page();
    let frame = "document.getElementById('f')";
    wait_for_value(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.__marker"),
        &serde_json::json!("one"),
    );

    // The tab has two used steps: the initial blank and the main document.
    // The child's initial blank shares the main step, so the iframe's first
    // load replaces it rather than adding one
    // (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#getting-all-used-history-steps>).
    assert_eq!(evaluate(&mut client, &session, "history.length"), 2);
    assert_eq!(
        evaluate(
            &mut client,
            &session,
            &format!("{frame}.contentWindow.history.length")
        ),
        2,
        "a child's length is the tab's step count, not its own entry list"
    );

    // A child navigation is one more joint step, and every frame sees it.
    evaluate(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.location.href = '/child2'"),
    );
    wait_for_value(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.__marker"),
        &serde_json::json!("two"),
    );
    assert_eq!(evaluate(&mut client, &session, "history.length"), 3);
    assert_eq!(
        evaluate(
            &mut client,
            &session,
            &format!("{frame}.contentWindow.history.length")
        ),
        3
    );

    // The parent's back() steps the tab to the entry before the child's
    // navigation, which puts the child back on its first document.
    evaluate(&mut client, &session, "history.back()");
    wait_for_value(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.__marker"),
        &serde_json::json!("one"),
    );
    assert_eq!(
        evaluate(
            &mut client,
            &session,
            &format!("{frame}.contentWindow.history.length")
        ),
        3,
        "back moves the position, not the step count"
    );

    evaluate(&mut client, &session, "history.forward()");
    wait_for_value(
        &mut client,
        &session,
        &format!("{frame}.contentWindow.__marker"),
        &serde_json::json!("two"),
    );
}
