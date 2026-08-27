//! ureq [`Connector`] that dials through [`crate::dial::open`].

use std::fmt;
use std::io::{Read, Write};
use std::sync::Mutex;

use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, LazyBuffers, NextTimeout, Transport,
};
use url::Url;

use crate::dial::{self, RawStream};

/// DNS stays in [`dial::open`]; ureq still requires a resolver step before
/// the connector runs, so this returns a dummy address and never looks up
/// the origin host (needed so `http://origin.test` through a proxy
/// does not fail DNS).
#[derive(Debug, Default)]
pub(crate) struct DialResolver;

impl Resolver for DialResolver {
    fn resolve(
        &self,
        _uri: &ureq::http::Uri,
        _config: &ureq::config::Config,
        _timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        Ok(self.empty())
    }
}

/// Shared-dial connector: TCP + CONNECT + TLS live in [`dial::open`].
#[derive(Clone, Debug)]
pub(crate) struct NetConnector {
    pub(crate) proxy: Option<String>,
    pub(crate) timeout: Option<std::time::Duration>,
}

impl Connector for NetConnector {
    type Out = StreamTransport;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<()>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        if chained.is_some() {
            return Err(ureq::Error::ConnectionFailed);
        }
        let url =
            Url::parse(&details.uri.to_string()).map_err(|_| ureq::Error::ConnectionFailed)?;
        let stream = dial::open(&url, self.proxy.as_deref(), self.timeout).map_err(to_ureq)?;
        let buffers = LazyBuffers::new(
            details.config.input_buffer_size(),
            details.config.output_buffer_size(),
        );
        Ok(Some(StreamTransport {
            stream: Mutex::new(stream),
            buffers,
        }))
    }
}

pub(crate) struct StreamTransport {
    stream: Mutex<RawStream>,
    buffers: LazyBuffers,
}

impl Transport for StreamTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        let mut stream = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apply_timeout(&stream, timeout, true)?;
        let output = &self.buffers.output()[..amount];
        stream
            .write_all(output)
            .map_err(|err| map_io(err, timeout))?;
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let mut stream = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apply_timeout(&stream, timeout, false)?;
        let input = self.buffers.input_append_buf();
        let amount = stream.read(input).map_err(|err| map_io(err, timeout))?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        let stream = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stream.peek_open()
    }

    fn is_tls(&self) -> bool {
        self.stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_tls()
    }
}

impl fmt::Debug for StreamTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamTransport").finish_non_exhaustive()
    }
}

fn apply_timeout(stream: &RawStream, timeout: NextTimeout, write: bool) -> Result<(), ureq::Error> {
    let dur = match timeout.not_zero() {
        Some(ureq::unversioned::transport::time::Duration::Exact(d)) => Some(d),
        _ => None,
    };
    if write {
        stream.set_write_timeout(dur).map_err(ureq::Error::from)?;
    } else {
        stream.set_read_timeout(dur).map_err(ureq::Error::from)?;
    }
    Ok(())
}

fn map_io(err: std::io::Error, timeout: NextTimeout) -> ureq::Error {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            ureq::Error::Timeout(timeout.reason)
        }
        _ => ureq::Error::from(err),
    }
}

fn to_ureq(err: crate::NetError) -> ureq::Error {
    match err {
        crate::NetError::Transport(crate::TransportError::Io(e)) => ureq::Error::Io(e),
        crate::NetError::Transport(crate::TransportError::Tls(detail)) => {
            ureq::Error::Io(std::io::Error::other(crate::error::DialTlsFailure(detail)))
        }
        crate::NetError::Transport(crate::TransportError::Connect(_)) => {
            ureq::Error::ConnectionFailed
        }
        crate::NetError::Transport(crate::TransportError::Dns(_)) => ureq::Error::HostNotFound,
        crate::NetError::Transport(crate::TransportError::Timeout(kind)) => {
            let t = match kind {
                crate::TimeoutKind::PerCall => ureq::Timeout::PerCall,
                crate::TimeoutKind::Connect => ureq::Timeout::Connect,
                crate::TimeoutKind::Global
                | crate::TimeoutKind::Resolve
                | crate::TimeoutKind::SendRequest
                | crate::TimeoutKind::SendBody
                | crate::TimeoutKind::RecvResponse
                | crate::TimeoutKind::RecvBody
                | crate::TimeoutKind::Unknown(_) => ureq::Timeout::Global,
            };
            ureq::Error::Timeout(t)
        }
        crate::NetError::Protocol(crate::ProtocolError::InvalidProxy) => {
            ureq::Error::InvalidProxyUrl
        }
        _ => ureq::Error::ConnectionFailed,
    }
}
