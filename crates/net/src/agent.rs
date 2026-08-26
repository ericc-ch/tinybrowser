//! [`Agent`] and [`AgentBuilder`]: connection ownership and config.

use std::time::Duration;

use crate::method::Method;
use crate::request::RequestBuilder;
use url::Url;

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

    /// Time budget that resets at each backend call (ureq's per-call knob).
    /// Redirect following is not this crate's job; `browser` owns that loop.
    #[must_use]
    pub fn timeout_per_call(mut self, timeout: Duration) -> Self {
        self.timeout_per_call = Some(timeout);
        self
    }

    /// Materialize the agent.
    ///
    /// The backend seam lives here too: this is the only place ureq's
    /// config is shaped (`http_status_as_error(false)` is decision 02's
    /// statuses-as-data, set once at construction).
    #[must_use]
    pub fn build(self) -> Agent {
        // Decision 02: net sends only headers we build ourselves. Every
        // backend-injected default is suppressed — a stray `ureq/3.4.0`
        // user-agent, an `Accept-Encoding: gzip` we cannot honor, or an
        // `HTTP_PROXY` picked up from the process environment would be
        // wire behavior we don't own. Ticket 08's explicit proxy knob is
        // still ahead; until it lands, proxy stays off. UA layering lives
        // in `send()`, so the backend auto-header is None here too.
        // Fetch-accurate extension methods (`patch`, `propfind`) need
        // `allow_non_standard_methods`: ureq's default `verify_version`
        // whitelist would otherwise refuse them.
        // Redirect following belongs to `browser` (fetch
        // #http-redirect-fetch). The backend returns 3xx as data
        // (`max_redirects(0)`).
        let config = ureq::config::Config::builder()
            .http_status_as_error(false)
            .max_redirects(0)
            // Option-valued knobs pass straight through; None keeps backend
            // defaults (no timeouts configured).
            .timeout_global(self.timeout_global)
            .timeout_per_call(self.timeout_per_call)
            .user_agent(ureq::config::AutoHeaderValue::None)
            .accept(ureq::config::AutoHeaderValue::None)
            .accept_encoding(ureq::config::AutoHeaderValue::None)
            .proxy(None)
            .allow_non_standard_methods(true)
            .build();
        Agent {
            inner: config.new_agent(),
            ua: self.user_agent,
        }
    }
}

/// One connection pool plus its config: the object callers hold to dial.
///
/// Cheap to clone (the backend pool is shared through an interior Arc);
/// each clone dials through the same pool.
#[derive(Clone, Debug)]
pub struct Agent {
    pub(super) inner: ureq::Agent,
    pub(super) ua: Option<String>,
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
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}
