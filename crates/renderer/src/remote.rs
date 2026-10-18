//! Value-only script results that may cross the browser seam.

use serde::{Deserialize, Serialize};

/// JSON-shaped script result. No DOM handles or `QuickJS` values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RemoteValue {
    /// JS `undefined`.
    Undefined,
    /// JS `null`.
    Null,
    /// JS boolean.
    Bool(bool),
    /// JS number, including `NaN` and `Infinity`.
    Number(f64),
    /// JS string.
    String(String),
    /// JS array.
    List(Vec<RemoteValue>),
    /// JS object as insertion-ordered entries.
    Map(Vec<(String, RemoteValue)>),
    /// Interned node identity allocated on the renderer.
    Node(u64),
}
