//! Browser-owned live networking: one [`net::Agent`] behind a value-only handle.
//!
//! Tab coordinators receive [`FetchHandle`]. They do not expose or own
//! [`net::Agent`].

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use net::{Agent, AgentBuilder, InitiatorKind, Method};
use renderer::{DialFailure, DialOutcome, DialRequest};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

use crate::profile::ProfileName;
use crate::store::ProfileStore;

/// Default per-call fetch timeout on a [`FetchHandle`].
pub(crate) const PAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on a navigation body.
pub(crate) const NAV_BODY_LIMIT: usize = 1_048_576;

/// One completed navigation dial.
pub(crate) struct NavOutcome {
    pub status: u16,
    pub final_url: Url,
    pub content_type: Option<String>,
    pub content_language: Option<String>,
    pub body: net::Body,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Browser-owned live networking service for one Profile.
///
/// Wraps one shared [`Agent`]: connection pool, transport settings, and the
/// live cookie jar. [`ProfileStore`] is durable backing, not a second jar.
pub struct NetworkSession {
    agent: Agent,
    store: Arc<ProfileStore>,
    permits: NetworkPermits,
}

impl NetworkSession {
    /// Session from `builder`, with the default per-call fetch timeout.
    ///
    /// # Errors
    ///
    /// Stored profile data could not be read or quarantined.
    pub fn from_builder(builder: AgentBuilder, store: ProfileStore) -> io::Result<Self> {
        Self::from_agent(builder.timeout_per_call(PAGE_FETCH_TIMEOUT).build(), store)
    }

    /// Session that shares `agent` (and therefore its cookie jar).
    ///
    /// # Errors
    ///
    /// Stored profile data could not be read or quarantined.
    pub fn from_agent(agent: Agent, store: ProfileStore) -> io::Result<Self> {
        store.load_into(&agent)?;
        Ok(Self {
            agent,
            store: Arc::new(store),
            permits: NetworkPermits::new(),
        })
    }

    /// Value-only fetch handle for a tab coordinator, with its own per-tab cap.
    #[must_use]
    pub(crate) fn fetch_handle(&self) -> FetchHandle {
        FetchHandle {
            agent: self.agent.clone(),
            store: Arc::clone(&self.store),
            permits: self.permits.clone(),
            tab: Arc::new(Semaphore::new(MAX_TAB_DIALS)),
        }
    }

    pub(crate) async fn persist(&self) -> io::Result<()> {
        let store = Arc::clone(&self.store);
        let agent = self.agent.clone();
        tokio::task::spawn_blocking(move || store.save_from(&agent))
            .await
            .map_err(io::Error::other)?
    }

    pub(crate) fn profile_name(&self) -> ProfileName {
        self.store.profile_name().clone()
    }

    /// Cookies visible to `url`, including session and `HttpOnly` cookies.
    pub(crate) fn cookie_records(&self, url: &Url) -> Vec<net::CookieRecord> {
        self.agent.cookie_records(url)
    }

    /// Drops every cookie from the live jar.
    pub(crate) fn clear_cookies(&self) {
        self.agent.clear_cookies();
    }

    /// Stores one `Set-Cookie` line for `url`.
    pub(crate) fn add_cookie(&self, cookie: &str, url: &Url) {
        self.agent.set_cookie(cookie, url);
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

/// Cloneable, sendable handle for cookies and async HTTP.
///
/// Completions return to the tab coordinator as renderer events.
#[derive(Clone)]
pub(crate) struct FetchHandle {
    agent: Agent,
    store: Arc<ProfileStore>,
    permits: NetworkPermits,
    tab: Arc<Semaphore>,
}

impl FetchHandle {
    /// `document.cookie` getter for `url`.
    #[must_use]
    pub(crate) fn cookies_for(&self, url: &Url) -> String {
        self.agent.cookies_for(url)
    }

    /// `document.cookie` setter for `url`.
    pub(crate) fn set_cookie(&self, value: &str, url: &Url) {
        self.agent.set_cookie(value, url);
        self.store.mark_dirty();
    }

    pub(crate) fn request(&self, method: Method, url: Url) -> net::RequestBuilder {
        self.agent.request(method, url)
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
        let fetch = self.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                biased;
                _ = cancel.changed() => Err(DialFailure::Cancelled),
                result = fetch.navigate(&url, &initiator) => result,
            };
            let _send_result = reply.send((epoch, result));
        })
    }

    async fn navigate(&self, url: &Url, initiator: &Url) -> Result<NavOutcome, DialFailure> {
        let deadline = Instant::now() + PAGE_FETCH_TIMEOUT;
        let permits = Self::acquire(&self.permits.navigation, deadline).await?;
        let response = self
            .request(Method::GET, url.clone())
            .with_initiator_kind(InitiatorKind::Navigation)
            .with_initiator(initiator.clone())
            .deadline(deadline)
            .send()
            .await
            .map_err(|error| dial_failure(&error))?;
        self.store.mark_dirty();
        let status = response.status();
        let final_url = response.final_url().clone();
        let content_language = response
            .headers()
            .get("content-language")
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(content_language_tag);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .map(str::to_owned);
        let body = response.into_body();
        Ok(NavOutcome {
            status,
            final_url,
            content_type,
            content_language,
            body,
            _permit: permits,
        })
    }

    /// Async GET for one renderer service call, cancelled when the renderer
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
            let response = self
                .request(Method::GET, url)
                .with_initiator_kind(InitiatorKind::Fetch)
                .with_initiator(initiator.clone())
                .deadline(deadline)
                .send()
                .await
                .map_err(|error| dial_failure(&error))?;
            self.store.mark_dirty();
            let status = response.status();
            let final_url = response.final_url().to_string();
            let content_language = response
                .headers()
                .get("content-language")
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .and_then(content_language_tag);
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .map(str::to_owned);
            let mut body = Vec::new();
            if request.read_body {
                let mut response_body = response.into_body();
                while let Some(chunk) = response_body
                    .read_chunk()
                    .await
                    .map_err(|error| dial_failure(&error))?
                {
                    if body.len().saturating_add(chunk.len()) > NAV_BODY_LIMIT {
                        return Err(DialFailure::Limit);
                    }
                    body.extend_from_slice(&chunk);
                }
            }
            Ok(DialOutcome {
                status,
                final_url,
                content_type,
                content_language,
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

/// One `Content-Language` tag, or `None` when the header lists several
/// languages ([HTML document language](https://html.spec.whatwg.org/multipage/dom.html#language)).
fn content_language_tag(raw: &str) -> Option<String> {
    let mut tags = raw
        .split(',')
        .map(|part| part.split(';').next().unwrap_or(part).trim())
        .filter(|tag| !tag.is_empty());
    let first = tags.next()?.to_owned();
    if tags.next().is_some() {
        return None;
    }
    Some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn exhausted_permits_fail_with_queue_full_before_the_deadline() {
        let permits = Arc::new(Semaphore::new(1));
        let _held = Arc::clone(&permits).acquire_owned().await.expect("permit");

        let error = FetchHandle::acquire(&permits, Instant::now() + Duration::from_millis(50))
            .await
            .expect_err("second permit must time out");
        assert_eq!(error, DialFailure::QueueFull);
    }

    #[tokio::test]
    async fn released_permits_are_reusable() {
        let permits = Arc::new(Semaphore::new(1));
        {
            let _held = Arc::clone(&permits).acquire_owned().await.expect("permit");
            let error = FetchHandle::acquire(&permits, Instant::now() + Duration::from_millis(20))
                .await
                .expect_err("held permit");
            assert_eq!(error, DialFailure::QueueFull);
        }
        let _again = FetchHandle::acquire(&permits, Instant::now() + Duration::from_millis(50))
            .await
            .expect("permit after release");
    }
}
