//! tinybrowser's network layer: bytes in, bytes out, cookies managed.
//!
//! Charter ([ADR 0006](../../docs/adr/0006-net-transport.md), decisions in
//! `.scratch/net-crate/`): a small sync client owning its entire public
//! type surface. v1 dials through ureq 3 + native-tls (ticket 12 wires
//! TLS); the deferred stealth milestone replaces the backend with a
//! hand-rolled `BoringSSL` stack — and **no consumer changes**, because of
//! the hard seam below.
//!
//! # The hard seam
//!
//! Every public type is ours: [`Agent`], [`RequestBuilder`], [`Response`],
//! [`Body`], [`HeaderMap`], [`Method`], [`Context`], [`NetError`]. Backend
//! types exist only inside `AgentBuilder::build`, [`RequestBuilder::send`],
//! [`Response::from_backend`], and `From<ureq::Error>` — never in a
//! signature. Statuses are data, bodies stream, cancellation is drop.
//! `send()` is one hop; `browser` follows redirects.
//!
//! # Seam map
//!
//! ```text
//! browser (fan-in):  navigation + injected fetch/XHR/WebSocket over Agent
//! js:                HttpTransport trait defined in js, implemented in browser
//! net:               dial, stream, cookie jar above the transport
//! ```
//!
//! Sync everywhere (`std::net`, no tokio). Blocking IO is the default
//! until a decision says otherwise.

mod agent;
mod context;
mod error;
mod header;
mod method;
mod request;
mod response;
mod token;

pub use agent::{Agent, AgentBuilder};
pub use context::Context;
pub use error::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};
pub use header::{HeaderError, HeaderMap};
pub use method::{InvalidMethod, Method};
pub use request::RequestBuilder;
pub use response::{Body, Response};
