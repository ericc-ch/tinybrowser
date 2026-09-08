#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Context {
    Navigation,
    #[default]
    Fetch,
    Xhr,
    WsHandshake,
}
