//! [`Context`]: the initiator riding every request.

/// What kind of initiator is behind a request.
///
/// Two consumers of this fact (decision 02): `SameSite` uses this enum for
/// the Lax top-level-navigation exception, and the post-swap stealth
/// milestone emits canonical `Sec-Fetch-*` headers that differ by context.
/// Schemeful same-site is initiator URL versus request URL, not this enum
/// alone. The slot costs nothing now; retrofitting it would touch every
/// consumer.
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
