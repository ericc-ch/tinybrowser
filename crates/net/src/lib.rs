//! tinybrowser's network layer: bytes in, bytes out, cookies managed.
//!
//! Charter ([ADR 0006](../../wiki/adrs/0006-net-transport.md)): a small sync
//! client owning its entire public type surface. v1 dials through ureq 3 +
//! native-tls; the deferred stealth milestone replaces the backend with a
//! hand-rolled `BoringSSL` stack — and **no consumer changes**, because of
//! the hard seam below.
//!
//! # The hard seam
//!
//! Every public type is ours: [`Agent`], [`RequestBuilder`], [`Response`],
//! [`Body`], [`HeaderMap`], [`Method`], [`Context`], [`NetError`],
//! [`WebSocket`]. Backend types exist only inside `AgentBuilder::build`,
//! [`RequestBuilder::send`], [`RequestBuilder::upgrade`],
//! [`Response::from_backend`], `From<ureq::Error>`, `dial::open`, and
//! `NetConnector` — never in a signature. Statuses are data, bodies stream,
//! cancellation is drop. `send()` follows redirects (Chrome cap 20; ureq's
//! table stays off). HTTP and WebSocket share one outbound builder:
//! [`RequestBuilder::send`] and [`RequestBuilder::upgrade`].
//!
//! # Seam map
//!
//! ```text
//! browser:  navigation + fetch/XHR/WebSocket over Agent (holds Agent)
//! net:      dial, stream, cookie jar
//! ```
//!
//! Blocking `send()` / `upgrade`. The page thread must call them through
//! Tokio `spawn_blocking` ([ADR 0007](../../wiki/adrs/0007-engine-charter.md)).

mod agent;
mod connector;
mod context;
mod cookie;
mod dial;
mod error;
mod header;
mod method;
mod request;
mod response;
mod token;
mod websocket;

pub use agent::{Agent, AgentBuilder};
pub use context::Context;
pub use error::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};
pub use header::{HeaderError, HeaderMap};
pub use method::{InvalidMethod, Method};
pub use request::RequestBuilder;
pub use response::{Body, Response};
pub use websocket::{WebSocket, WsEvent, WsMessage};
