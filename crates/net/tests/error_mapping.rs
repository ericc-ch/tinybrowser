use std::io;

use net::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};

#[test]
fn backend_errors_map_exhaustively_into_the_public_error_model() {
    for (source, expected) in [
        (ureq::Error::HostNotFound, "dns"),
        (ureq::Error::ConnectionFailed, "connect"),
        (ureq::Error::ConnectProxyFailed("denied".into()), "connect"),
        (ureq::Error::Tls("expired"), "tls"),
    ] {
        let actual = match NetError::from(source) {
            NetError::Transport(TransportError::Dns(_)) => "dns",
            NetError::Transport(TransportError::Connect(_)) => "connect",
            NetError::Transport(TransportError::Tls(_)) => "tls",
            other => panic!("unexpected transport mapping: {other:?}"),
        };
        assert_eq!(actual, expected);
    }

    let source = io::Error::new(io::ErrorKind::BrokenPipe, "socket died");
    assert!(matches!(
        NetError::from(ureq::Error::Io(source)),
        NetError::Transport(TransportError::Io(error))
            if error.kind() == io::ErrorKind::BrokenPipe
    ));

    for (source, expected) in [
        (ureq::Timeout::Global, TimeoutKind::Global),
        (ureq::Timeout::PerCall, TimeoutKind::PerCall),
        (ureq::Timeout::Resolve, TimeoutKind::Resolve),
        (ureq::Timeout::Connect, TimeoutKind::Connect),
        (ureq::Timeout::SendRequest, TimeoutKind::SendRequest),
        (ureq::Timeout::SendBody, TimeoutKind::SendBody),
        (ureq::Timeout::RecvResponse, TimeoutKind::RecvResponse),
        (ureq::Timeout::RecvBody, TimeoutKind::RecvBody),
    ] {
        assert!(matches!(
            NetError::from(ureq::Error::Timeout(source)),
            NetError::Transport(TransportError::Timeout(actual)) if actual == expected
        ));
    }
    assert!(matches!(
        NetError::from(ureq::Error::Timeout(ureq::Timeout::Await100)),
        NetError::Transport(TransportError::Timeout(TimeoutKind::Unknown(name)))
            if name.contains("Await100")
    ));

    assert!(matches!(
        NetError::from(ureq::Error::TooManyRedirects),
        NetError::Limit(LimitExceeded::Redirect)
    ));
    assert!(matches!(
        NetError::from(ureq::Error::BodyExceedsLimit(2048)),
        NetError::Limit(LimitExceeded::Size(2048))
    ));
    assert!(matches!(
        NetError::from(ureq::Error::LargeResponseHeader(9999, 1024)),
        NetError::Limit(LimitExceeded::Size(1024))
    ));

    for source in [
        ureq::Error::StatusCode(500),
        ureq::Error::BadUri("missing scheme".into()),
        ureq::Error::RedirectFailed,
        ureq::Error::InvalidProxyUrl,
    ] {
        assert!(matches!(
            NetError::from(source),
            NetError::Protocol(ProtocolError::Other(reason)) if !reason.is_empty()
        ));
    }
}
