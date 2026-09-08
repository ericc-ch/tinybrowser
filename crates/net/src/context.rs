/// Which initiator owns a request.
///
/// Cookie `SameSite` rules use this for the Lax top-level navigation exception.
/// Future `Sec-Fetch-*` headers also key off it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Context {
    /// Top-level navigation. This is the default for [`RequestBuilder`].
    #[default]
    Navigation,
    /// Scripted `fetch()`.
    Fetch,
    /// Scripted `XMLHttpRequest`.
    Xhr,
    /// WebSocket handshake.
    WsHandshake,
}
