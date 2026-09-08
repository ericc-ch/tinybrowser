use std::io;

use crate::context::Context;
use crate::error::{LimitExceeded, NetError, TransportError};
use crate::header::HeaderMap;
use url::Url;

const CHUNK_SIZE: usize = 16 * 1024;

pub struct Body {
    inner: Box<dyn io::Read + Send>,
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Body").finish_non_exhaustive()
    }
}

impl Body {
    pub(super) fn from_reader(inner: impl io::Read + Send + 'static) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    pub fn read_chunk(&mut self) -> Result<Option<Vec<u8>>, NetError> {
        let mut buf = vec![0u8; CHUNK_SIZE];
        loop {
            match self.inner.read(&mut buf) {
                Ok(0) => return Ok(None),
                Ok(n) => {
                    buf.truncate(n);
                    return Ok(Some(buf));
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(NetError::Transport(TransportError::Io(err))),
            }
        }
    }

    pub fn bytes(mut self, limit: usize) -> Result<Vec<u8>, NetError> {
        let mut out = Vec::new();
        while let Some(chunk) = self.read_chunk()? {
            if out.len() + chunk.len() > limit {
                return Err(NetError::Limit(LimitExceeded::Size(limit as u64)));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    pub fn text(self, limit: usize) -> Result<String, NetError> {
        self.bytes(limit)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[derive(Debug)]
pub struct Response {
    status: u16,
    headers: HeaderMap,
    final_url: Url,
    context: Context,
    body: Body,
}

impl Response {
    pub(super) fn from_backend(
        response: ureq::http::Response<ureq::Body>,
        context: Context,
        final_url: Url,
    ) -> Result<Self, NetError> {
        let status = response.status().as_u16();

        let mut headers = HeaderMap::new();
        for (name, value) in response.headers() {
            headers
                .insert(name.as_str(), value.as_bytes())
                .map_err(|_| {
                    NetError::Protocol(crate::error::ProtocolError::UnrepresentableHeader)
                })?;
        }

        let body = Body::from_reader(response.into_body().into_reader());

        Ok(Self {
            status,
            headers,
            final_url,
            context,
            body,
        })
    }

    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    #[must_use]
    pub fn final_url(&self) -> &Url {
        &self.final_url
    }

    #[must_use]
    pub fn context(&self) -> Context {
        self.context
    }

    #[must_use]
    pub fn into_body(self) -> Body {
        self.body
    }
}
