//! Opt-in live smokes for ticket 12. Offline `cargo test` must stay green
//! (decision 10): these stay `#[ignore]` and run with
//! `cargo test -p net --test live -- --ignored --nocapture`.

use net::{Agent, Method};

fn live_get(url: &str) -> net::Response {
    let parsed = url::Url::parse(url).expect("absolute");
    Agent::new()
        .request(Method::GET, parsed)
        .send()
        .unwrap_or_else(|err| panic!("live GET {url} failed: {err}"))
}

#[test]
#[ignore = "live network; ticket 12 example.com smoke"]
fn example_com_https_round_trips() {
    let response = live_get("https://example.com/");
    assert_eq!(response.status(), 200);
    let body = response
        .into_body()
        .text(64 * 1024)
        .expect("example.com body");
    assert!(
        body.contains("Example Domain"),
        "example.com page text missing from body"
    );
}

#[test]
#[ignore = "live network; ticket 12 peet.ws JA4 drift check"]
fn peet_ws_ja4_is_the_openssl_native_tls_shape() {
    // Drift detection, not a gate pass. v1 native-tls/OpenSSL ClientHello
    // is recorded as `t13d3011_…` in wiki/researches/size-budget.md (2026-08-25).
    let response = live_get("https://tls.peet.ws/api/all");
    assert_eq!(response.status(), 200);
    let body = response
        .into_body()
        .text(1024 * 1024)
        .expect("peet.ws body");
    assert!(
        body.contains("t13d3011"),
        "JA4 drifted off OpenSSL native-tls prefix t13d3011; body was: {body}"
    );
}
