//! A configured `--tls-ca` is what makes an unknown loopback certificate
//! trustworthy.
//!
//! The certificate is self-signed for `127.0.0.1`, so it is both the server's
//! identity and the client's only trust anchor. Without the anchor the dial
//! must fail as a TLS transport error; with it the same dial must reach the
//! request handler. The CLI test in `tests/browser/webdriver.rs` proves the
//! same through a whole browser, so what this adds is the exact failure kind,
//! `NetError::Transport(TransportError::Tls(_))`, with no process in the way
//! (<https://docs.rs/native-tls/latest/native_tls/struct.TlsConnector.html>).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use native_tls::{Identity, TlsAcceptor};
use net::{Agent, AgentOptions, Method, NetError, Request, TransportError, TrustedCa};

/// The self-signed loopback identity the browser tests also dial
/// (`tests/fixtures/tls/README.md` explains how to regenerate it).
const TLS_CERT_PEM: &str =
    include_str!("../../../../tests/fixtures/tls/loopback-127.0.0.1.crt.pem");
const TLS_KEY_PEM: &str = include_str!("../../../../tests/fixtures/tls/loopback-127.0.0.1.key.pem");

/// One HTTPS loopback server that serves `responses` completed requests.
///
/// A client that does not trust the certificate fails its handshake, and the
/// server stays up for the next connection: that refusal is half of what this
/// test measures.
fn https_server(responses: usize) -> (url::Url, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let addr = listener.local_addr().expect("local addr");
    let identity = Identity::from_pkcs8(TLS_CERT_PEM.as_bytes(), TLS_KEY_PEM.as_bytes())
        .expect("the test identity parses");
    let acceptor = TlsAcceptor::new(identity).expect("tls acceptor");
    let handle = std::thread::spawn(move || {
        let mut served = 0;
        while served < responses {
            let (stream, _) = listener.accept().expect("accept");
            // A peer that connects and sends nothing must not park the
            // handshake for the rest of the run.
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("read timeout is settable");
            stream
                .set_write_timeout(Some(Duration::from_secs(10)))
                .expect("write timeout is settable");
            let Ok(mut stream) = acceptor.accept(stream) else {
                continue;
            };
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).expect("request readable");
            assert!(read > 0, "peer closed before sending a request");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .expect("response writable");
            served += 1;
        }
    });
    (
        url::Url::parse(&format!("https://{addr}/")).expect("url"),
        handle,
    )
}

fn navigation(url: &url::Url) -> Request {
    let mut request = Request::new(Method::GET, url.clone());
    request.cors = net::cors::CorsSettings {
        mode: net::cors::RequestMode::Navigate,
        credentials: net::cors::CredentialsMode::Include,
        redirect: net::cors::RedirectMode::Follow,
        origin: url.origin(),
    };
    request
}

#[tokio::test]
async fn a_configured_ca_is_what_makes_an_unknown_loopback_certificate_trustworthy() {
    let (url, server) = https_server(1);

    let without = agent(Vec::new())
        .send(navigation(&url))
        .await
        .expect_err("an unknown self-signed certificate is not trustworthy");
    assert!(
        matches!(without, NetError::Transport(TransportError::Tls(_))),
        "{without:?}"
    );

    let ca = TrustedCa::from_pem(TLS_CERT_PEM.as_bytes()).expect("the test CA parses");
    let trusted = agent(vec![ca]);
    let response = trusted
        .send(navigation(&url))
        .await
        .expect("the configured CA makes the same dial trustworthy");
    assert_eq!(response.status(), 200);

    server.join().expect("server");
}

/// An agent that cannot wait forever: a broken fixture fails the test instead
/// of hanging it.
fn agent(tls_cas: Vec<TrustedCa>) -> Agent {
    Agent::new(AgentOptions {
        tls_cas,
        timeout_global: Some(Duration::from_secs(10)),
        ..AgentOptions::default()
    })
}
