use std::io::{self, Cursor};

use http1::{MAX_BODY, MAX_HEAD, Request, read_message, read_request, write_response};

#[test]
fn read_get_with_headers() {
    let mut stream = Cursor::new(b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let request = read_request(&mut stream).expect("request");
    assert_eq!(
        request,
        Request {
            method: "GET".into(),
            path: "/json/version".into(),
            headers: vec![("Host".into(), "127.0.0.1".into())],
            body: Vec::new(),
        }
    );
    assert_eq!(request.header("host"), Some("127.0.0.1"));
}

#[test]
fn read_switching_protocols() {
    let mut stream =
        Cursor::new(b"HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: abc\r\n\r\n");
    let message = read_message(&mut stream).expect("message");
    assert_eq!(message.start, "HTTP/1.1 101 Switching Protocols");
    assert_eq!(message.header("sec-websocket-accept"), Some("abc"));
}

#[test]
fn read_post_body_and_write_close() {
    let mut stream = Cursor::new(b"POST /session HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}");
    let request = read_request(&mut stream).expect("request");
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/session");
    assert_eq!(request.body, b"{}");

    let mut out = Vec::new();
    write_response(
        &mut out,
        200,
        "application/json; charset=utf-8",
        b"{\"value\":null}",
    )
    .expect("write");
    assert_eq!(
        out,
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: 14\r\nConnection: close\r\n\r\n{\"value\":null}"
    );
}

#[test]
fn reject_oversized_head_and_body() {
    let mut stream = Cursor::new(vec![b'A'; MAX_HEAD]);
    let err = read_request(&mut stream).expect_err("head cap");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);

    let too_long = format!(
        "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    );
    let err = read_request(&mut Cursor::new(too_long.into_bytes())).expect_err("body cap");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}
