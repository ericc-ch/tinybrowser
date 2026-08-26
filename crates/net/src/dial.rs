//! Shared dial: TCP, HTTP CONNECT, native-tls.
//!
//! HTTP `send()` and WebSocket upgrades both call [`open`]. The deferred
//! stealth swap replaces this function only.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use native_tls::{TlsConnector, TlsStream};
use url::Url;

use crate::error::{NetError, ProtocolError, TimeoutKind, TransportError};

/// TCP plus unread bytes that arrived with the CONNECT response head.
pub(crate) struct Socket {
    tcp: TcpStream,
    prefix: Vec<u8>,
}

impl Socket {
    fn new(tcp: TcpStream, prefix: Vec<u8>) -> Self {
        Self { tcp, prefix }
    }
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.prefix.is_empty() {
            let n = buf.len().min(self.prefix.len());
            buf[..n].copy_from_slice(&self.prefix[..n]);
            self.prefix.drain(..n);
            return Ok(n);
        }
        self.tcp.read(buf)
    }
}

impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tcp.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

/// Connected byte stream, TLS already applied when the URL is https/wss.
pub(crate) enum RawStream {
    Plain(Socket),
    Tls(TlsStream<Socket>),
}

impl RawStream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(s) => &s.tcp,
            Self::Tls(s) => &s.get_ref().tcp,
        }
    }

    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.tcp().set_read_timeout(timeout)
    }

    pub(crate) fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.tcp().set_write_timeout(timeout)
    }

    pub(crate) fn is_tls(&self) -> bool {
        matches!(self, Self::Tls(_))
    }

    /// Liveness probe that must not consume bytes (TLS records included).
    pub(crate) fn peek_open(&self) -> bool {
        if let Self::Plain(s) = self
            && !s.prefix.is_empty()
        {
            return true;
        }
        let tcp = self.tcp();
        if tcp.set_nonblocking(true).is_err() {
            return false;
        }
        let mut buf = [0];
        let open = match tcp.peek(&mut buf) {
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                true
            }
            Ok(0) | Err(_) => false,
            Ok(_) => true,
        };
        let _ = tcp.set_nonblocking(false);
        open
    }
}

impl Read for RawStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl Write for RawStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

/// Dial `url`, applying the optional HTTP CONNECT proxy and TLS.
pub(crate) fn open(
    url: &Url,
    proxy: Option<&str>,
    timeout: Option<Duration>,
) -> Result<RawStream, NetError> {
    let host = url
        .host_str()
        .ok_or(NetError::Protocol(ProtocolError::RejectedRequest))?;
    let tls = matches!(url.scheme(), "https" | "wss");
    let port = url.port_or_known_default().unwrap_or(if tls { 443 } else { 80 });
    let socket = if let Some(proxy) = proxy {
        connect_via_proxy(proxy, host, port, timeout)?
    } else {
        Socket::new(connect_tcp(host, port, timeout)?, Vec::new())
    };
    let _ = socket.tcp.set_nodelay(true);
    if tls {
        socket
            .tcp
            .set_read_timeout(timeout)
            .map_err(map_connect_io)?;
        socket
            .tcp
            .set_write_timeout(timeout)
            .map_err(map_connect_io)?;
        let connector = TlsConnector::new().map_err(|err| {
            NetError::Transport(TransportError::Tls(err.to_string().into()))
        })?;
        let tls_stream = match connector.connect(host, socket) {
            Ok(s) => s,
            Err(native_tls::HandshakeError::Failure(err)) => {
                return Err(NetError::Transport(TransportError::Tls(
                    err.to_string().into(),
                )));
            }
            Err(_) => {
                return Err(NetError::Transport(TransportError::Tls(
                    "tls handshake interrupted".into(),
                )));
            }
        };
        Ok(RawStream::Tls(tls_stream))
    } else {
        Ok(RawStream::Plain(socket))
    }
}

fn connect_tcp(host: &str, port: u16, timeout: Option<Duration>) -> Result<TcpStream, NetError> {
    let addrs = (host, port).to_socket_addrs().map_err(|_| {
        NetError::Transport(TransportError::Dns(host.into()))
    })?;
    let mut last = None;
    for addr in addrs {
        let result = match timeout {
            Some(limit) => TcpStream::connect_timeout(&addr, limit),
            None => TcpStream::connect(addr),
        };
        match result {
            Ok(stream) => return Ok(stream),
            Err(err) => last = Some(err),
        }
    }
    match last {
        Some(err) => Err(map_connect_io(err)),
        None => Err(NetError::Transport(TransportError::Dns(host.into()))),
    }
}

fn connect_via_proxy(
    proxy: &str,
    host: &str,
    port: u16,
    timeout: Option<Duration>,
) -> Result<Socket, NetError> {
    let proxy_url =
        Url::parse(proxy).map_err(|_| NetError::Protocol(ProtocolError::InvalidProxy))?;
    let phost = proxy_url
        .host_str()
        .ok_or(NetError::Protocol(ProtocolError::InvalidProxy))?;
    let proxy_port = proxy_url.port_or_known_default().unwrap_or(80);
    let mut stream = connect_tcp(phost, proxy_port, timeout)?;
    stream.set_read_timeout(timeout).map_err(map_connect_io)?;
    stream.set_write_timeout(timeout).map_err(map_connect_io)?;
    let authority = connect_authority(host, port);
    let mut req = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if !proxy_url.username().is_empty() {
        let password = proxy_url.password().unwrap_or("");
        let token = base64_basic(&format!("{}:{password}", proxy_url.username()));
        req.push_str("Proxy-Authorization: Basic ");
        req.push_str(&token);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).map_err(map_connect_io)?;
    stream.flush().map_err(map_connect_io)?;
    let mut buf = Vec::new();
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let leftover = buf[end + 4..].to_vec();
            let head = String::from_utf8_lossy(&buf[..end + 4]);
            let status = head
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(0);
            if status != 200 {
                return Err(NetError::Transport(TransportError::Connect(
                    format!("CONNECT {status}").into(),
                )));
            }
            return Ok(Socket::new(stream, leftover));
        }
        if buf.len() > 64 * 1024 {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        let mut chunk = [0u8; 512];
        let n = stream.read(&mut chunk).map_err(map_connect_io)?;
        if n == 0 {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn connect_authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn base64_basic(input: &str) -> String {
    const ALPH: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes.get(i + 1).copied();
        let b2 = bytes.get(i + 2).copied();
        out.push(ALPH[(b0 >> 2) as usize] as char);
        out.push(ALPH[(((b0 & 0x03) << 4) | (b1.unwrap_or(0) >> 4)) as usize] as char);
        if b1.is_some() {
            out.push(
                ALPH[(((b1.unwrap_or(0) & 0x0f) << 2) | (b2.unwrap_or(0) >> 6)) as usize] as char,
            );
        } else {
            out.push('=');
        }
        if b2.is_some() {
            out.push(ALPH[(b2.unwrap_or(0) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

fn map_connect_io(err: std::io::Error) -> NetError {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            NetError::Transport(TransportError::Timeout(TimeoutKind::Connect))
        }
        _ => NetError::Transport(TransportError::Io(err)),
    }
}
