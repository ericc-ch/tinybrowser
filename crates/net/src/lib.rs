//! Blocking HTTP and WebSocket client for the page engine.
//!
//! Types in this crate are the public network surface. Behavior follows
//! [Fetch](https://fetch.spec.whatwg.org/) for HTTP and the WebSocket protocol
//! for `ws`/`wss`.

mod client;
mod context;
mod cookie;
mod error;
mod protocol;
mod resolve;
mod transport;
mod websocket;

pub use client::{Agent, AgentBuilder, Body, RequestBuilder, Response};
pub use context::Context;
pub use cookie::{CookieRecord, CookieSameSite};
pub use error::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};
pub use protocol::{HeaderError, HeaderMap, InvalidMethod, Method};
pub use websocket::{WebSocket, WsEvent, WsMessage};
