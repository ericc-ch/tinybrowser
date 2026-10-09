//! Browser-owned live networking: one [`net::Agent`] behind a value-only handle.
//!
//! Tab coordinators receive [`TabNetworkHandle`]. They do not expose or own
//! [`net::Agent`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use net::{Agent, AgentOptions, InitiatorKind, Method, Request};
use renderer::{DialFailure, DialKind, DialOutcome, DialRequest};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

/// Default per-call fetch timeout on a [`TabNetworkHandle`].
pub(crate) const PAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on one body carried inside a JSON control reply.
///
/// A top-level navigation document streams under
/// [`renderer::MAX_RESPONSE_BODY_BYTES`]. This limit is the control reply,
/// including a child frame whose response is returned in that reply.
pub(crate) const CONTROL_BODY_LIMIT: usize = 1_048_576;

/// One completed navigation dial.
pub(crate) struct NavOutcome {
    pub status: u16,
    pub final_url: Url,
    pub content_type: Option<String>,
    pub body: net::Body,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Live network state for one storage partition.
pub(crate) struct NetworkContext {
    agent: Agent,
    permits: NetworkPermits,
}

impl NetworkContext {
    /// Builds the partition network from caller `options`. Defaults fill only
    /// where the caller left a field `None`: caller wins on `user_agent` and
    /// `timeout_per_call`.
    ///
    /// # Errors
    ///
    /// [`Agent::new`] rejects a proxy, resolve spec, or TLS CA value.
    pub(crate) fn new(mut options: AgentOptions) -> Result<Self, net::NetError> {
        if options.user_agent.is_none() {
            options.user_agent = Some(crate::USER_AGENT.to_string());
        }
        if options.timeout_per_call.is_none() {
            options.timeout_per_call = Some(PAGE_FETCH_TIMEOUT);
        }
        let agent = Agent::new(options)?;
        Ok(Self {
            agent,
            permits: NetworkPermits::new(),
        })
    }

    /// Network capability for one tab, with its own per-tab cap.
    #[must_use]
    pub(crate) fn tab_handle(&self) -> TabNetworkHandle {
        TabNetworkHandle {
            agent: self.agent.clone(),
            permits: self.permits.clone(),
            tab: Arc::new(Semaphore::new(MAX_TAB_DIALS)),
        }
    }

    pub(crate) fn agent(&self) -> Agent {
        self.agent.clone()
    }

    /// Cookies visible to `url`, including session and `HttpOnly` cookies.
    pub(crate) fn cookie_records(&self, url: &Url) -> Vec<net::CookieRecord> {
        self.agent.cookie_records(url)
    }

    /// Drops every cookie from the live jar.
    pub(crate) fn clear_cookies(&self) {
        self.agent.clear_cookies();
    }

    /// Stores one `Set-Cookie` line for `url` with HTTP-level rules,
    /// returning whether it was stored.
    pub(crate) fn add_cookie(&self, cookie: &str, url: &Url) -> bool {
        self.agent.store_cookie_http(cookie, url)
    }
}

/// Limits for in-flight dials: browser-wide and per reserved class.
#[derive(Clone)]
struct NetworkPermits {
    global: Arc<Semaphore>,
    navigation: Arc<Semaphore>,
}

impl NetworkPermits {
    fn new() -> Self {
        Self {
            global: Arc::new(Semaphore::new(MAX_GLOBAL_DIALS)),
            navigation: Arc::new(Semaphore::new(MAX_NAVIGATION_DIALS)),
        }
    }
}

const MAX_GLOBAL_DIALS: usize = 32;
const MAX_NAVIGATION_DIALS: usize = 8;
const MAX_TAB_DIALS: usize = 8;

/// Cloneable network capability for one tab.
///
/// Completions return to the tab coordinator as renderer events.
#[derive(Clone)]
pub(crate) struct TabNetworkHandle {
    agent: Agent,
    permits: NetworkPermits,
    tab: Arc<Semaphore>,
}

impl TabNetworkHandle {
    /// `document.cookie` getter for `url`.
    #[must_use]
    pub(crate) fn cookies_for(&self, url: &Url) -> String {
        self.agent.cookies_for(url)
    }

    /// `document.cookie` setter for `url`.
    pub(crate) fn set_cookie(&self, value: &str, url: &Url) {
        self.agent.set_cookie(value, url);
    }

    /// Spawns one navigation dial and returns its task handle. The caller
    /// aborts the handle to cancel the request. The completion returns on
    /// `reply` tagged with `epoch`.
    pub(crate) fn dial_navigation(
        &self,
        epoch: u64,
        url: Url,
        initiator: Url,
        reply: UnboundedSender<(u64, Result<NavOutcome, DialFailure>)>,
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let network = self.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                biased;
                _ = cancel.changed() => Err(DialFailure::Cancelled),
                result = network.navigate(&url, &initiator) => result,
            };
            let _send_result = reply.send((epoch, result));
        })
    }

    async fn navigate(&self, url: &Url, initiator: &Url) -> Result<NavOutcome, DialFailure> {
        let deadline = Instant::now() + PAGE_FETCH_TIMEOUT;
        let permits = Self::acquire(&self.permits.navigation, deadline).await?;
        let mut request = Request::new(Method::GET, url.clone());
        chrome_navigation_request(&mut request, url);
        request.initiator_kind = InitiatorKind::Navigation;
        request.initiator = Some(initiator.clone());
        request.deadline = Some(deadline);
        let response = self
            .agent
            .send(request)
            .await
            .map_err(|error| dial_failure(&error))?;
        let status = response.status();
        let final_url = response.final_url().clone();
        let content_type = content_type_header(response.headers());
        let body = response.into_body();
        Ok(NavOutcome {
            status,
            final_url,
            content_type,
            body,
            _permit: permits,
        })
    }

    /// Async HTTP request for one renderer service call, cancelled when the renderer
    /// dies or the tab closes.
    pub(crate) async fn dial_request(
        &self,
        request: &DialRequest,
        initiator: &Url,
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<DialOutcome, DialFailure> {
        let call = async {
            let deadline = Instant::now() + PAGE_FETCH_TIMEOUT;
            let _global = Self::acquire(&self.permits.global, deadline).await?;
            let _tab = Self::acquire(&self.tab, deadline).await?;
            let url = Url::parse(&request.url).map_err(|_| DialFailure::Connect)?;
            // Until the Fetch CORS protocol is implemented, do not issue a
            // cross-origin script request (including requests with side
            // effects) or expose a redirected response to the caller.
            // <https://fetch.spec.whatwg.org/#http-fetch>
            if request.kind == DialKind::JsFetch && url.origin() != initiator.origin() {
                return Err(DialFailure::Connect);
            }
            let method = net::Method::parse(&request.method).map_err(|_| DialFailure::Connect)?;
            // https://fetch.spec.whatwg.org/#forbidden-method
            if request.kind == DialKind::JsFetch
                && matches!(
                    method.as_str().to_ascii_uppercase().as_str(),
                    "CONNECT" | "TRACE" | "TRACK"
                )
            {
                return Err(DialFailure::Connect);
            }
            let mut outbound = Request::new(method, url);
            if request.kind == DialKind::JsFetch {
                outbound.same_origin = Some(initiator.origin());
            }
            if let Some(content_type) = &request.content_type {
                let _ = outbound
                    .headers
                    .insert("Content-Type", content_type.as_bytes());
            }
            for (name, value) in &request.headers {
                if request.kind == DialKind::JsFetch && forbidden_page_header(name, value) {
                    continue;
                }
                outbound
                    .headers
                    .insert(name, value.as_bytes())
                    .map_err(|_| DialFailure::Connect)?;
            }
            if let Some(referrer) = &request.referrer {
                // Defense in depth: the renderer computes this per
                // `referrer_for_policy`, but the network boundary re-validates
                // so a compromised renderer cannot leak a cross-origin `Referer`.
                if is_safe_referrer(referrer, &request.url) {
                    outbound
                        .headers
                        .insert("Referer", referrer.as_bytes())
                        .map_err(|_| DialFailure::Connect)?;
                }
            }
            if !request.body.is_empty() {
                outbound.body = Some(request.body.clone());
            }
            outbound.initiator_kind = InitiatorKind::Fetch;
            outbound.initiator = Some(initiator.clone());
            outbound.deadline = Some(deadline);
            let response = self
                .agent
                .send(outbound)
                .await
                .map_err(|error| dial_failure(&error))?;
            let status = response.status();
            let final_url = response.final_url().to_string();
            let content_type = content_type_header(response.headers());
            let headers = response
                .headers()
                .iter()
                .filter(|(name, _)| {
                    request.kind != DialKind::JsFetch
                        || (!name.eq_ignore_ascii_case("set-cookie")
                            && !name.eq_ignore_ascii_case("set-cookie2"))
                })
                .map(|(name, value)| (name.to_owned(), String::from_utf8_lossy(value).into_owned()))
                .collect();
            let body = if request.read_body {
                response
                    .into_body()
                    .bytes(CONTROL_BODY_LIMIT)
                    .await
                    .map_err(|error| dial_failure(&error))?
            } else {
                Vec::new()
            };
            Ok(DialOutcome {
                status,
                final_url,
                content_type,
                headers,
                body,
            })
        };
        tokio::select! {
            biased;
            _ = cancel.changed() => Err(DialFailure::Cancelled),
            result = call => result,
        }
    }

    async fn acquire(
        semaphore: &Arc<Semaphore>,
        deadline: Instant,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, DialFailure> {
        let permit = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            Arc::clone(semaphore).acquire_owned(),
        )
        .await;
        match permit {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(DialFailure::Cancelled),
            Err(_) => Err(DialFailure::QueueFull),
        }
    }
}

/// Browser-generated request headers cannot be overridden by page scripts.
/// <https://fetch.spec.whatwg.org/#forbidden-request-header>
fn forbidden_page_header(name: &str, value: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("proxy-") || lower.starts_with("sec-") {
        return true;
    }
    if matches!(
        lower.as_str(),
        "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
            | "connection"
            | "content-length"
            | "cookie"
            | "cookie2"
            | "date"
            | "dnt"
            | "expect"
            | "host"
            | "keep-alive"
            | "origin"
            | "referer"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "via"
    ) {
        return true;
    }
    matches!(
        lower.as_str(),
        "x-http-method" | "x-http-method-override" | "x-method-override"
    ) && value.split(',').any(|method| {
        matches!(
            method.trim().to_ascii_lowercase().as_str(),
            "connect" | "trace" | "track"
        )
    })
}

pub(crate) fn dial_failure(error: &net::NetError) -> DialFailure {
    use net::{NetError, TransportError};
    match error {
        NetError::Transport(TransportError::Dns(_)) => DialFailure::Dns,
        NetError::Transport(TransportError::Tls(_)) => DialFailure::Tls,
        NetError::Transport(TransportError::Timeout(_)) => DialFailure::Timeout,
        NetError::Limit(_) => DialFailure::Limit,
        NetError::Transport(TransportError::Connect(_) | TransportError::Io(_))
        | NetError::Protocol(_) => DialFailure::Connect,
    }
}

/// Navigation identity headers for Chrome emulation.
///
/// `Upgrade-Insecure-Requests` is the document navigation preference
/// (<https://www.w3.org/TR/upgrade-insecure-requests/#preference>).
/// `Accept` and Fetch Metadata (`Sec-Fetch-*`) match Chrome's document
/// navigation request
/// (<https://w3c.github.io/webappsec-fetch-metadata/>).
/// Low-entropy UA client hints are sent only to potentially trustworthy URLs
/// (<https://wicg.github.io/ua-client-hints/#sec-ch-ua>,
/// <https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>).
fn chrome_navigation_request(request: &mut Request, url: &Url) {
    identity_header(request, "Upgrade-Insecure-Requests", "1");
    identity_header(
        request,
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
    );
    identity_header(request, "Sec-Fetch-Site", "none");
    identity_header(request, "Sec-Fetch-Mode", "navigate");
    identity_header(request, "Sec-Fetch-User", "?1");
    identity_header(request, "Sec-Fetch-Dest", "document");
    if sends_default_ua_client_hints(url) {
        for (name, value) in [
            ("Sec-CH-UA", crate::SEC_CH_UA),
            ("Sec-CH-UA-Mobile", crate::SEC_CH_UA_MOBILE),
            ("Sec-CH-UA-Platform", crate::SEC_CH_UA_PLATFORM),
            (
                "Sec-CH-Prefers-Color-Scheme",
                crate::SEC_CH_PREFERS_COLOR_SCHEME,
            ),
        ] {
            identity_header(request, name, value);
        }
    }
}

fn identity_header(request: &mut Request, name: &str, value: &str) {
    request
        .headers
        .insert(name, value)
        .expect("Chrome navigation identity headers are static HTTP tokens with no CTL bytes");
}

fn sends_default_ua_client_hints(url: &Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    match url.host() {
        Some(url::Host::Ipv4(addr)) => addr.is_loopback(),
        Some(url::Host::Ipv6(addr)) => addr.is_loopback(),
        Some(url::Host::Domain(host)) => {
            let host = host.to_ascii_lowercase();
            host == "localhost" || host.ends_with(".localhost")
        }
        None => false,
    }
}

/// `Content-Type` from response `headers`, when it is UTF-8.
fn content_type_header(headers: &net::HeaderMap) -> Option<String> {
    headers
        .get("content-type")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::to_owned)
}

/// Network-boundary `Referer` check: same-origin, or cross-origin only to a
/// trustworthy (https/loopback) target without downgrade. The renderer already
/// applies the policy; this keeps a compromised renderer from leaking.
fn is_safe_referrer(referrer: &str, request_url: &str) -> bool {
    let (Ok(from), Ok(to)) = (Url::parse(referrer), Url::parse(request_url)) else {
        return false;
    };
    if !matches!(from.scheme(), "http" | "https") {
        return false;
    }
    let same_origin = from.scheme() == to.scheme()
        && from.host_str() == to.host_str()
        && from.port_or_known_default() == to.port_or_known_default();
    if same_origin {
        return true;
    }
    if from.scheme() == "https" && matches!(to.scheme(), "http" | "ws") {
        return false;
    }
    matches!(to.scheme(), "https" | "wss")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn exhausted_permits_fail_with_queue_full_before_the_deadline() {
        let permits = Arc::new(Semaphore::new(1));
        let _held = Arc::clone(&permits).acquire_owned().await.expect("permit");

        let error = TabNetworkHandle::acquire(&permits, Instant::now() + Duration::from_millis(50))
            .await
            .expect_err("second permit must time out");
        assert_eq!(error, DialFailure::QueueFull);
    }

    #[tokio::test]
    async fn released_permits_are_reusable() {
        let permits = Arc::new(Semaphore::new(1));
        {
            let _held = Arc::clone(&permits).acquire_owned().await.expect("permit");
            let error =
                TabNetworkHandle::acquire(&permits, Instant::now() + Duration::from_millis(20))
                    .await
                    .expect_err("held permit");
            assert_eq!(error, DialFailure::QueueFull);
        }
        let _again =
            TabNetworkHandle::acquire(&permits, Instant::now() + Duration::from_millis(50))
                .await
                .expect("permit after release");
    }

    #[test]
    fn default_ua_client_hints_are_limited_to_trustworthy_urls() {
        assert!(sends_default_ua_client_hints(
            &Url::parse("https://example.com/").expect("https")
        ));
        assert!(!sends_default_ua_client_hints(
            &Url::parse("http://example.com/").expect("http")
        ));
        assert!(sends_default_ua_client_hints(
            &Url::parse("http://127.0.0.1/").expect("loopback")
        ));
        assert!(sends_default_ua_client_hints(
            &Url::parse("http://localhost/").expect("localhost")
        ));
    }
}
