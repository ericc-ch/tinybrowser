//! Inbound HTTP/1.1 for loopback protocol adapters.
//!
//! CDP discovery and `WebDriver` REST share a start-line, headers, and a
//! `Content-Length` body. WebSocket upgrade and JSON encoding stay in those
//! crates. This is not the outbound `net` client.

use std::io::{self, Read, Write};

/// Bytes allowed before `\r\n\r\n`.
pub const MAX_HEAD: usize = 65_536;

/// Bytes allowed for a `Content-Length` body.
pub const MAX_BODY: usize = 8_388_608;

/// One HTTP/1.1 request or response after the head and optional body.
#[derive(Debug, PartialEq, Eq)]
pub struct Message {
    /// Request line or status line, without the trailing `\r\n`.
    pub start: String,
    /// Header names and values, trimmed, in wire order.
    pub headers: Vec<(String, String)>,
    /// Body of `Content-Length` bytes, or empty.
    pub body: Vec<u8>,
}

impl Message {
    /// First header named `name`, ASCII case-insensitive.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        named_header(&self.headers, name)
    }
}

/// One HTTP/1.1 request after the head and optional body have been read.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    /// Method token from the request line (`GET`, `POST`, …).
    pub method: String,
    /// Request-target from the request line, usually an origin-form path.
    pub path: String,
    /// Header names and values, trimmed, in wire order.
    pub headers: Vec<(String, String)>,
    /// Body of `Content-Length` bytes, or empty.
    pub body: Vec<u8>,
}

impl Request {
    /// First header named `name`, ASCII case-insensitive.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        named_header(&self.headers, name)
    }
}

fn named_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Reads one HTTP/1.1 message: head through `\r\n\r\n`, then `Content-Length` bytes.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidData`] when the head exceeds [`MAX_HEAD`] or the
/// declared body exceeds [`MAX_BODY`]. [`io::ErrorKind::UnexpectedEof`] when
/// the stream closes before the head or body is complete.
pub fn read_message(stream: &mut impl Read) -> io::Result<Message> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() >= MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let read = stream.read(&mut byte)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "closed during head",
            ));
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let start = lines.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    if content_length > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request body too large",
        ));
    }
    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body)?;
    }
    Ok(Message {
        start,
        headers,
        body,
    })
}

/// Reads one HTTP/1.1 request: head through `\r\n\r\n`, then `Content-Length` bytes.
///
/// # Errors
///
/// Same conditions as [`read_message`].
pub fn read_request(stream: &mut impl Read) -> io::Result<Request> {
    let message = read_message(stream)?;
    let mut parts = message.start.split_whitespace();
    Ok(Request {
        method: parts.next().unwrap_or("").to_owned(),
        path: parts.next().unwrap_or("/").to_owned(),
        headers: message.headers,
        body: message.body,
    })
}

/// Writes `HTTP/1.1` status, `Content-Type`, `Content-Length`, and `Connection: close`.
///
/// # Errors
///
/// Returns when writing to `stream` fails.
pub fn write_response(
    stream: &mut impl Write,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}
