//! Value-only script results that may cross [`crate::PageHandle`].

/// JSON-shaped script result. No DOM handles or `QuickJS` values.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteValue {
    /// JS `undefined` or `null`.
    Null,
    /// JS boolean.
    Bool(bool),
    /// JS number.
    Number(f64),
    /// JS string.
    String(String),
    /// JS array.
    List(Vec<RemoteValue>),
    /// JS object as insertion-ordered entries.
    Map(Vec<(String, RemoteValue)>),
    /// Interned node identity allocated on the page actor.
    Node(u64),
}
