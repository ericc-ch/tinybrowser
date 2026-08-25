//! [`Context`]: the initiator riding every request.

/// What kind of initiator is behind a request.
///
/// Two consumers of this fact (decision 02): `SameSite` cookie decisions are
/// a function of initiator context (Firefox staples `LoadInfo` +
/// `CookieJarSettings` onto every load), and the post-swap stealth
/// milestone emits canonical `Sec-Fetch-*` headers that differ by context.
/// The slot costs nothing now; retrofitting it would touch every consumer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Context {
    /// Top-level document navigation.
    #[default]
    Navigation,
    /// Page JavaScript `fetch()`.
    Fetch,
    /// Page JavaScript `XMLHttpRequest`.
    Xhr,
    /// The HTTP Upgrade handshake behind a WebSocket dial.
    WsHandshake,
}
