//! [`Agent`] and [`AgentBuilder`]: connection ownership and config.

use std::time::Duration;

use std::sync::{Arc, Mutex};

use crate::context::Context;
use crate::cookie::{CookieJar, CookieOp, RetrievalKind};
use crate::error::{NetError, ProtocolError};
use crate::header::HeaderMap;
use crate::method::Method;
use crate::request::RequestBuilder;
use url::Url;

/// Chrome's follow cap (fetch #http-redirect-fetch uses 20; matching the
/// fingerprint target costs nothing versus ureq's default of 10).
const DEFAULT_MAX_REDIRECTS: u32 = 20;

/// Builder for [`Agent`] configuration (decision 02: config lives on the
/// agent; per-request overrides deferred until a consumer demands them).
///
/// Every knob defaults to a decided value — constructing with
/// [`AgentBuilder::new`] and building immediately yields the policy the
/// decisions recorded.
#[derive(Clone, Debug)]
pub struct AgentBuilder {
    /// Verbatim `User-Agent` header injected into every request. `None`
    /// means net sends no UA of its own.
    user_agent: Option<String>,
    timeout_global: Option<Duration>,
    timeout_per_call: Option<Duration>,
    max_redirects: u32,
    /// HTTP CONNECT authority, parsed by [`AgentBuilder::proxy`]. `None`
    /// means no proxy (and environment `HTTP_PROXY` stays ignored).
    proxy: Option<String>,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBuilder {
    /// A builder carrying only decided defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            user_agent: None,
            timeout_global: None,
            timeout_per_call: None,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            proxy: None,
        }
    }

    /// Set the `User-Agent` sent with every request from this agent.
    ///
    /// Default, not law: a request-level `User-Agent` header
    /// ([`RequestBuilder::header`](crate::RequestBuilder::header))
    /// overrides it — the agent value is not stacked with one the
    /// request already set. Repeated request-level `User-Agent`
    /// entries still append (RFC 9110 §5.2).
    #[must_use]
    pub fn user_agent(mut self, value: &str) -> Self {
        self.user_agent = Some(value.to_owned());
        self
    }

    /// End-to-end time budget per call, including body reads.
    #[must_use]
    pub fn timeout_global(mut self, timeout: Duration) -> Self {
        self.timeout_global = Some(timeout);
        self
    }

    /// Time budget that resets at each backend call (ureq's per-call knob),
    /// including each redirect hop `send()` follows.
    #[must_use]
    pub fn timeout_per_call(mut self, timeout: Duration) -> Self {
        self.timeout_per_call = Some(timeout);
        self
    }

    /// How many `Location` hops `send()` will follow.
    ///
    /// Default **20** (Chrome / fetch #http-redirect-fetch). `0` means a
    /// 3xx with `Location` is itself the final response (decision 02).
    /// Exceeding a positive cap yields
    /// [`NetError::Limit`](crate::NetError::Limit)`(`
    /// [`LimitExceeded::Redirect`](crate::LimitExceeded::Redirect)`)`.
    #[must_use]
    pub fn max_redirects(mut self, max_redirects: u32) -> Self {
        self.max_redirects = max_redirects;
        self
    }

    /// Route every dial through an HTTP CONNECT proxy.
    ///
    /// The string is an `http://` authority (`http://user:pass@host:port`
    /// or `http://host:port`). SOCKS stays off until measured (ticket 08);
    /// environment `HTTP_PROXY` is never read.
    ///
    /// # Errors
    ///
    /// [`NetError::Protocol`](crate::NetError::Protocol)`(`
    /// [`ProtocolError::InvalidProxy`](crate::ProtocolError::InvalidProxy)`)`
    /// when the string is not a usable `http://` URI.
    pub fn proxy(mut self, authority: &str) -> Result<Self, NetError> {
        let parsed = Url::parse(authority).map_err(|_| invalid_proxy())?;
        if parsed.scheme() != "http" || parsed.host_str().is_none() {
            return Err(invalid_proxy());
        }
        // The backend parser is the conversion point for this knob: if it
        // cannot represent a URI we already accepted as `http://host`, the
        // caller still sees InvalidProxy rather than a later send() surprise.
        ureq::Proxy::new(authority).map_err(|_| invalid_proxy())?;
        self.proxy = Some(authority.to_owned());
        Ok(self)
    }

    /// Materialize the agent.
    ///
    /// The backend seam lives here too: this is the only place ureq's
    /// config is shaped (`http_status_as_error(false)` is decision 02's
    /// statuses-as-data, set once at construction). Redirect following is
    /// ours (`max_redirects(0)` on the backend); TLS is native-tls, never
    /// the rustls default ureq would pick if a feature leaked in.
    #[must_use]
    pub fn build(self) -> Agent {
        // Decision 02: net sends only headers we build ourselves. Every
        // backend-injected default is suppressed — a stray `ureq/3.4.0`
        // user-agent, an `Accept-Encoding: gzip` we cannot honor, or an
        // `HTTP_PROXY` picked up from the process environment would be
        // wire behavior we don't own. UA layering lives in `send()`, so
        // the backend auto-header is None here too.
        // Fetch-accurate extension methods (`patch`, `propfind`) need
        // `allow_non_standard_methods`: ureq's default `verify_version`
        // whitelist would otherwise refuse them.
        // Proxy is applied in [`crate::dial::open`], not by ureq — otherwise
        // ureq would CONNECT while we also dial the proxy.
        let config = ureq::config::Config::builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(self.timeout_global)
            .timeout_per_call(self.timeout_per_call)
            .user_agent(ureq::config::AutoHeaderValue::None)
            .accept(ureq::config::AutoHeaderValue::None)
            .accept_encoding(ureq::config::AutoHeaderValue::None)
            .proxy(None)
            .allow_non_standard_methods(true)
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::NativeTls)
                    .build(),
            )
            .build();
        let inner = ureq::Agent::with_parts(
            config,
            crate::connector::NetConnector {
                proxy: self.proxy.clone(),
                timeout: self.timeout_per_call.or(self.timeout_global),
            },
            crate::connector::DialResolver,
        );
        Agent {
            inner,
            ua: self.user_agent,
            max_redirects: self.max_redirects,
            jar: Arc::new(Mutex::new(CookieJar::default())),
            proxy: self.proxy,
            timeout: self.timeout_per_call.or(self.timeout_global),
        }
    }
}

fn invalid_proxy() -> NetError {
    NetError::Protocol(ProtocolError::InvalidProxy)
}

/// One connection pool plus its config: the object callers hold to dial.
///
/// Cheap to clone (the backend pool is shared through an interior Arc);
/// each clone dials through the same pool.
#[derive(Clone)]
pub struct Agent {
    pub(super) inner: ureq::Agent,
    pub(super) ua: Option<String>,
    pub(super) max_redirects: u32,
    pub(super) jar: Arc<Mutex<CookieJar>>,
    pub(super) proxy: Option<String>,
    pub(super) timeout: Option<Duration>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("max_redirects", &self.max_redirects)
            .field("has_proxy", &self.proxy.is_some())
            .field("jar", &self.jar)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// An agent with decided defaults ([`AgentBuilder::new`] then build).
    #[must_use]
    pub fn new() -> Self {
        AgentBuilder::new().build()
    }

    /// Start a request to an absolute URL.
    ///
    /// The URL must be absolute by construction: the entry type is
    /// [`url::Url`], whose no-base parser rejects relative input (WHATWG
    /// URL standard, "basic URL parser" — relative input without a base
    /// is a failure). Relative resolution against the document or
    /// `<base>` belongs to `browser`, upstream of this crate.
    #[must_use]
    pub fn request(&self, method: Method, url: Url) -> RequestBuilder {
        RequestBuilder::new(self.clone(), method, url)
    }

    /// Cookie-string for `uri` using the non-HTTP jar API
    /// ([RFC 6265bis §5.8.2](https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-non-http-apis)):
    /// `HttpOnly` cookies are omitted; `Secure` cookies omit on cleartext URIs.
    /// Page `document.cookie` in `browser` calls this; it is not the DOM property.
    #[must_use]
    pub fn cookies_for(&self, uri: &Url) -> String {
        self.jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cookie_string(CookieOp {
                url: uri,
                now: std::time::SystemTime::now(),
                kind: RetrievalKind::NonHttp,
                context: Context::Navigation,
                method: &Method::GET,
                initiator: Some(uri),
            })
    }

    /// Store one cookie line against `uri` using the non-HTTP jar API
    /// ([RFC 6265bis §5.8.2](https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-non-http-apis)).
    /// Invalid or `HttpOnly` input is ignored. Page `document.cookie` assignment
    /// in `browser` calls this.
    pub fn set_cookie(&self, value: &str, uri: &Url) {
        self.jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .store(
                value,
                CookieOp {
                    url: uri,
                    now: std::time::SystemTime::now(),
                    kind: RetrievalKind::NonHttp,
                    context: Context::Navigation,
                    method: &Method::GET,
                    initiator: Some(uri),
                },
            );
    }

    /// Merge jar Cookie, agent User-Agent, and (for [`Context::WsHandshake`]
    /// with an initiator) Origin into `headers` before a wire dial.
    ///
    /// Shared by [`RequestBuilder::send`](crate::RequestBuilder::send) and
    /// [`RequestBuilder::upgrade`](crate::RequestBuilder::upgrade).
    pub(crate) fn prepare_outbound(
        &self,
        headers: &mut HeaderMap,
        url: &Url,
        context: Context,
        method: &Method,
        initiator: Option<&Url>,
    ) {
        let cookie = self
            .jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cookie_string(CookieOp {
                url,
                now: std::time::SystemTime::now(),
                kind: RetrievalKind::Http,
                context,
                method,
                initiator,
            });
        if !cookie.is_empty() {
            if let Some(existing) = headers.get("cookie") {
                let merged = format!("{}; {cookie}", String::from_utf8_lossy(existing));
                headers.remove("cookie");
                let _ = headers.insert("Cookie", merged.as_bytes());
            } else {
                let _ = headers.insert("Cookie", cookie.as_bytes());
            }
        }
        if headers.get("user-agent").is_none()
            && let Some(ua) = &self.ua
        {
            let _ = headers.insert("User-Agent", ua.as_bytes());
        }
        // Page WS sends Origin from the document; embedder dials omit it.
        if context == Context::WsHandshake
            && let Some(document) = initiator
        {
            let origin = document.origin().ascii_serialization();
            headers.remove("origin");
            let _ = headers.insert("Origin", origin.as_bytes());
        }
    }

    /// Harvest `Set-Cookie` lines already parsed at a response boundary.
    pub(crate) fn store_set_cookie_lines(
        &self,
        url: &Url,
        context: Context,
        method: &Method,
        initiator: Option<&Url>,
        lines: impl IntoIterator<Item = impl AsRef<str>>,
    ) {
        let now = std::time::SystemTime::now();
        let mut jar = self
            .jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for line in lines {
            jar.store(
                line.as_ref(),
                CookieOp {
                    url,
                    now,
                    kind: RetrievalKind::Http,
                    context,
                    method,
                    initiator,
                },
            );
        }
    }
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}
