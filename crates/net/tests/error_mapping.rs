//! The ureq→`NetError` conversion table: the seam the whole crate leans on.
//!
//! Ticket 10 names focused error-mapping units as testing priority 4.
//! Every constructible arm of `From<ureq::Error>` is pinned here, so a
//! backend upgrade that silently reclassifies a failure shows up in this
//! file first.

use std::io;

use net::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};

#[test]
fn transport_arms_map_faithfully() {
    fn expect_transport(err: ureq::Error) -> TransportError {
        match NetError::from(err) {
            NetError::Transport(cause) => cause,
            other => panic!("expected NetError::Transport, got {other:?}"),
        }
    }

    assert!(matches!(
        expect_transport(ureq::Error::HostNotFound),
        TransportError::Dns(_)
    ));
    assert!(matches!(
        expect_transport(ureq::Error::ConnectionFailed),
        TransportError::Connect(_)
    ));
    assert!(matches!(
        expect_transport(ureq::Error::ConnectProxyFailed("proxy said no".into())),
        TransportError::Connect(_)
    ));
    assert!(matches!(
        expect_transport(ureq::Error::Tls("certificate expired")),
        TransportError::Tls(_)
    ));

    // Io keeps its cause so callers can branch on ErrorKind.
    let io = io::Error::new(io::ErrorKind::BrokenPipe, "socket died");
    match expect_transport(ureq::Error::Io(io)) {
        TransportError::Io(err) => assert_eq!(err.kind(), io::ErrorKind::BrokenPipe),
        other => panic!("expected Transport(Io), got {other:?}"),
    }
}

#[test]
fn timeout_knobs_map_by_name_not_by_guess() {
    let knobs = [
        (ureq::Timeout::Global, TimeoutKind::Global),
        (ureq::Timeout::PerCall, TimeoutKind::PerCall),
        (ureq::Timeout::Resolve, TimeoutKind::Resolve),
        (ureq::Timeout::Connect, TimeoutKind::Connect),
        (ureq::Timeout::SendRequest, TimeoutKind::SendRequest),
        (ureq::Timeout::SendBody, TimeoutKind::SendBody),
        (ureq::Timeout::RecvResponse, TimeoutKind::RecvResponse),
        (ureq::Timeout::RecvBody, TimeoutKind::RecvBody),
    ];
    for (knob, want) in knobs {
        match NetError::from(ureq::Error::Timeout(knob)) {
            NetError::Transport(TransportError::Timeout(kind)) => {
                assert_eq!(kind, want, "{knob:?} must map to {want:?}");
            }
            other => panic!("expected Transport(Timeout({want:?})), got {other:?}"),
        }
    }
}

#[test]
fn unknown_timeout_knobs_carry_the_backend_spelling() {
    // `Await100` is #[doc(hidden)] upstream ("never seen outside ureq",
    // ureq 3.4 src/timings.rs) — exactly the shape a future-knob surprise
    // takes. The mapping must report what arrived, not fabricate a known
    // category.
    match NetError::from(ureq::Error::Timeout(ureq::Timeout::Await100)) {
        NetError::Transport(TransportError::Timeout(TimeoutKind::Unknown(name))) => {
            assert!(
                name.contains("Await100"),
                "unknown knobs keep their raw spelling, got {name:?}"
            );
        }
        other => panic!("expected Unknown timeout kind, got {other:?}"),
    }
}

#[test]
fn limit_arms_keep_the_cap_that_fired() {
    match ureq::Error::TooManyRedirects.into() {
        NetError::Limit(LimitExceeded::Redirect) => {}
        other => panic!("expected Limit(Redirect), got {other:?}"),
    }
    match ureq::Error::BodyExceedsLimit(2048).into() {
        NetError::Limit(LimitExceeded::Size(cap)) => assert_eq!(cap, 2048),
        other => panic!("expected Limit(Size(2048)), got {other:?}"),
    }
    // Received-header cap: same Size class, payload is the header cap.
    match ureq::Error::LargeResponseHeader(9999, 1024).into() {
        NetError::Limit(LimitExceeded::Size(cap)) => assert_eq!(cap, 1024),
        other => panic!("expected Limit(Size(1024)), got {other:?}"),
    }
}

#[test]
fn protocol_shaped_arms_land_in_protocol_with_reasons() {
    // StatusCode can never fire under http_status_as_error(false); the
    // mapping must still classify it sanely rather than panic.
    for err in [
        ureq::Error::StatusCode(500),
        ureq::Error::BadUri("missing scheme".into()),
        ureq::Error::RedirectFailed,
        ureq::Error::InvalidProxyUrl,
    ] {
        match NetError::from(err) {
            NetError::Protocol(ProtocolError::Other(reason)) => assert!(!reason.is_empty()),
            other => panic!("expected Protocol(Other), got {other:?}"),
        }
    }

    // Backend-rejected locally-built requests (http-crate failures).
    let http_err = ureq::http::Request::builder()
        .header("bad\u{0}name", "value")
        .body(())
        .expect_err("NUL in header name is an http::Error");
    match ureq::Error::Http(http_err).into() {
        NetError::Protocol(ProtocolError::RejectedRequest) => {}
        other => panic!("expected Protocol, got {other:?}"),
    }
}
