use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::context::Context;
use crate::cookie::{CookieJar, CookieOp, RetrievalKind};
use crate::error::{NetError, ProtocolError};
use crate::header::HeaderMap;
use crate::method::Method;
use crate::request::RequestBuilder;
use url::Url;

// https://fetch.spec.whatwg.org/#http-redirect-fetch
const DEFAULT_MAX_REDIRECTS: u32 = 20;

#[derive(Clone)]
pub struct AgentBuilder {
    user_agent: Option<String>,
    timeout_global: Option<Duration>,
    timeout_per_call: Option<Duration>,
    max_redirects: u32,
    proxy: Option<String>,
}

impl std::fmt::Debug for AgentBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBuilder")
            .field("has_proxy", &self.proxy.is_some())
            .field("timeout_global", &self.timeout_global)
            .field("timeout_per_call", &self.timeout_per_call)
            .field("max_redirects", &self.max_redirects)
            .finish_non_exhaustive()
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBuilder {
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

    #[must_use]
    pub fn user_agent(mut self, value: &str) -> Self {
        self.user_agent = Some(value.to_owned());
        self
    }

    #[must_use]
    pub fn timeout_global(mut self, timeout: Duration) -> Self {
        self.timeout_global = Some(timeout);
        self
    }

    #[must_use]
    pub fn timeout_per_call(mut self, timeout: Duration) -> Self {
        self.timeout_per_call = Some(timeout);
        self
    }

    #[must_use]
    pub fn max_redirects(mut self, max_redirects: u32) -> Self {
        self.max_redirects = max_redirects;
        self
    }

    pub fn proxy(mut self, authority: &str) -> Result<Self, NetError> {
        let parsed = Url::parse(authority).map_err(|_| invalid_proxy())?;
        if parsed.scheme() != "http" || parsed.host_str().is_none() {
            return Err(invalid_proxy());
        }
        self.proxy = Some(authority.to_owned());
        Ok(self)
    }

    #[must_use]
    pub fn build(self) -> Agent {
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
            now: SystemTime::now,
        }
    }
}

fn invalid_proxy() -> NetError {
    NetError::Protocol(ProtocolError::InvalidProxy)
}

#[derive(Clone)]
pub struct Agent {
    pub(super) inner: ureq::Agent,
    pub(super) ua: Option<String>,
    pub(super) max_redirects: u32,
    pub(super) jar: Arc<Mutex<CookieJar>>,
    pub(super) proxy: Option<String>,
    pub(super) timeout: Option<Duration>,
    now: fn() -> SystemTime,
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
    #[must_use]
    pub fn new() -> Self {
        AgentBuilder::new().build()
    }

    #[must_use]
    pub fn request(&self, method: Method, url: Url) -> RequestBuilder {
        RequestBuilder::new(self.clone(), method, url)
    }

    #[must_use]
    pub fn cookies_for(&self, uri: &Url) -> String {
        self.jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cookie_string(CookieOp {
                url: uri,
                now: (self.now)(),
                kind: RetrievalKind::NonHttp,
                context: Context::Fetch,
                method: &Method::GET,
                initiator: Some(uri),
            })
    }

    pub fn set_cookie(&self, value: &str, uri: &Url) {
        self.jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .store(
                value,
                CookieOp {
                    url: uri,
                    now: (self.now)(),
                    kind: RetrievalKind::NonHttp,
                    context: Context::Fetch,
                    method: &Method::GET,
                    initiator: Some(uri),
                },
            );
    }

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
                now: (self.now)(),
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
        if context == Context::WsHandshake
            && let Some(document) = initiator
        {
            let origin = document.origin().ascii_serialization();
            headers.remove("origin");
            let _ = headers.insert("Origin", origin.as_bytes());
        }
    }

    pub(crate) fn store_set_cookie_lines(
        &self,
        url: &Url,
        context: Context,
        method: &Method,
        initiator: Option<&Url>,
        lines: impl IntoIterator<Item = impl AsRef<str>>,
    ) {
        let now = (self.now)();
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
