//! [`Response`] and streaming [`Body`]: what a dial gives back.

use std::io;

use crate::context::Context;
use crate::error::{LimitExceeded, NetError, TransportError};
use crate::header::HeaderMap;
use url::Url;

/// Upper bound on one [`Body::read_chunk`] allocation.
const CHUNK_SIZE: usize = 16 * 1024;

/// A response body: a lazy stream of chunks.
///
/// Streaming-first by decision 02 — XHR progress events and fetch
/// streaming are chunk-shaped, and a buffered-only design would force a
/// consumer-visible breaking change at the stealth backend swap.
///
/// **Cancellation is dropping.** Dropping the `Response`/`Body` before
/// reading to end closes the underlying connection; that IS abort (the
/// backend never returns a mid-body connection to its pool). Reading to
/// end, by contrast, hands the connection back for reuse.
pub struct Body {
    /// The backend's reader behind an opaque trait object: no backend
    /// type ever appears in a signature (decision 01).
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

    /// Next chunk of body bytes, blocking until some are available.
    ///
    /// Returns `Ok(None)` at end of body. Chunk size is bounded by
    /// net-internal buffering, not by the message: callers loop until
    /// `None` to drain.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] when the socket dies mid-stream.
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

    /// Drain the whole body into memory, enforcing `limit` bytes.
    ///
    /// Consuming by design: after buffering, the stream is over.
    ///
    /// # Errors
    ///
    /// [`NetError::Limit(LimitExceeded::Size)`](NetError::Limit) carrying
    /// the caller's `limit` when the body exceeds it;
    /// [`NetError::Transport`] on socket failure.
    pub fn bytes(mut self, limit: usize) -> Result<Vec<u8>, NetError> {
        let mut out = Vec::new();
        while let Some(chunk) = self.read_chunk()? {
            if out.len() + chunk.len() > limit {
                // Lossless on this crate's x86_64 target; no `From<usize> for u64`.
                return Err(NetError::Limit(LimitExceeded::Size(limit as u64)));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    /// Drain the whole body as text, enforcing `limit` bytes.
    ///
    /// Invalid UTF-8 decodes with replacement characters rather than
    /// failing — the same default the WHATWG Encoding standard gives
    /// `TextDecoder` (`fatal: false`). No charset *sniffing* happens here:
    /// non-UTF-8 encodings are the parse pipeline's problem (decision 02),
    /// this only handles the bytes that claim to be UTF-8 already.
    ///
    /// # Errors
    ///
    /// Same as [`Body::bytes`].
    pub fn text(self, limit: usize) -> Result<String, NetError> {
        self.bytes(limit)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// One HTTP response: status as data, headers, final URL, streamed body.
///
/// Every HTTP status arrives here as data (decision 02): navigation renders
/// error pages like normal ones, matching WHATWG fetch where network errors
/// reject but statuses resolve (#http-network-or-cache-fetch).
///
/// Wire-fidelity caveats under the v1 backend (lowercased names, unordered
/// received-header iteration) are recorded once — in [`HeaderMap`]'s module
/// docs — not restated per method.
#[derive(Debug)]
pub struct Response {
    status: u16,
    headers: HeaderMap,
    final_url: Url,
    context: Context,
    body: Body,
}

impl Response {
    /// The single ureq→net conversion point for responses (decision 01).
    ///
    /// Backend types exist only inside this function. Received header
    /// names populate lowercase under v1 (all the backend exposes); the
    /// stealth swap upgrades fidelity without touching signatures.
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

    /// The HTTP status code, any of 200..=599 — data, not error.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Response headers, ASCII-case-insensitive lookup.
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The URL this response belongs to: the request URL, fragment included.
    /// The fragment never goes on the wire
    /// ([fetch #http-network-or-cache-fetch](https://fetch.spec.whatwg.org/#http-network-or-cache-fetch)).
    /// Redirect hops that change the URL are `browser`'s job; this crate
    /// reports the URL of the single hop it dialed.
    #[must_use]
    pub fn final_url(&self) -> &Url {
        &self.final_url
    }

    /// The initiator context this request was sent under.
    #[must_use]
    pub fn context(&self) -> Context {
        self.context
    }

    /// Take ownership of the streamed body.
    #[must_use]
    pub fn into_body(self) -> Body {
        self.body
    }
}
