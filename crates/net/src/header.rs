//! [`HeaderMap`]: case-preserving, insertion-order-preserving header
//! storage with ASCII-case-insensitive lookup (RFC 9110 §5.1).
//!
//! This type is part of the hard seam (decision 01): it exists so the
//! stealth backend swap can upgrade wire fidelity. Under the v1 backend,
//! wire names are lowercase in both directions and *received* header
//! iteration order is whatever the backend parsed (unordered) — but
//! *stored* case is kept exactly as callers set it, request order always
//! survives to the wire, and a future swap can upgrade both without
//! touching signatures. This module doc is the single home for that
//! fidelity story; other types point here.

use std::fmt;

use crate::token::is_token_char;

/// Why a header could not be stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// The name is not a valid field name (RFC 9110 §5.1 token grammar).
    InvalidName(Box<str>),
    /// The value contains bytes HTTP forbids: control characters other
    /// than HTAB (RFC 9110 §5.5 `field-value` allows only VCHAR, SP/HTAB,
    /// and obs-text). This is the header-injection guard.
    InvalidValue(Box<str>),
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(f, "invalid header name: {name:?}"),
            Self::InvalidValue(value) => write!(f, "invalid header value: {value:?}"),
        }
    }
}

impl std::error::Error for HeaderError {}

/// One stored header: name kept verbatim, value as raw bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    value: Vec<u8>,
}

/// An ordered multimap of headers.
///
/// - Insertion order is preserved and replayed by [`HeaderMap::iter`].
/// - Lookup ([`HeaderMap::get`]) is ASCII-case-insensitive per RFC 9110
///   §5.1 and returns the **first** value stored under a matching name.
/// - Duplicate names are legal (RFC 9110 §5.2 keeps repeated fields in
///   order); use [`HeaderMap::get_all`] to see every value.
///
/// Stored names keep the case the caller set; values keep their exact
/// bytes. The v1 backend normalizes wire names to lowercase both
/// directions — a fidelity caveat, not a signature change.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct HeaderMap {
    entries: Vec<Entry>,
}

impl HeaderMap {
    /// An empty map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `name: value`, appending to any existing entries.
    ///
    /// The name keeps its case exactly as given; values keep their exact
    /// bytes. Duplicate names are allowed and stay in order.
    ///
    /// Grammar ownership is deliberately split: this boundary rejects
    /// everything injection-shaped (names outside the §5.1 token set,
    /// values holding CTLs) so hostile bytes never reach the transport;
    /// the backend's fuller field-content grammar (e.g. leading/trailing
    /// SP, which RFC 9110 §5.6.1 trims on the wire anyway) still applies
    /// at [`send`](crate::RequestBuilder::send) time as
    /// [`NetError::Protocol`](crate::NetError::Protocol). Two rejection
    /// latencies for two different classes of garbage — by design, not by
    /// accident.
    ///
    /// # Errors
    ///
    /// [`HeaderError::InvalidName`] if `name` is empty or holds characters
    /// outside the RFC 9110 §5.1 token set; [`HeaderError::InvalidValue`]
    /// if `value` holds control characters outside the RFC 9110 §5.5
    /// field-value grammar (everything below 0x20 except HTAB, plus DEL).
    pub fn insert(&mut self, name: &str, value: impl Into<Vec<u8>>) -> Result<(), HeaderError> {
        if name.is_empty() || !name.chars().all(is_token_char) {
            return Err(HeaderError::InvalidName(name.into()));
        }
        let value = value.into();
        if value.iter().any(|&b| (b < 0x20 && b != b'\t') || b == 0x7F) {
            return Err(HeaderError::InvalidValue(
                String::from_utf8_lossy(&value).into_owned().into(),
            ));
        }
        self.entries.push(Entry {
            name: name.to_owned(),
            value,
        });
        Ok(())
    }

    /// Drop every entry whose name matches ASCII-case-insensitively.
    pub fn remove(&mut self, name: &str) {
        self.entries
            .retain(|entry| !entry.name.eq_ignore_ascii_case(name));
    }

    /// First value whose name matches ASCII-case-insensitively.
    ///
    /// Names that are not valid tokens simply match nothing — lookups are
    /// infallible by design.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.get_all(name).next()
    }

    /// Every value whose name matches ASCII-case-insensitively, in
    /// insertion order.
    pub fn get_all<'s>(&'s self, name: &str) -> impl Iterator<Item = &'s [u8]> {
        self.entries
            .iter()
            .filter(move |entry| entry.name.eq_ignore_ascii_case(name))
            .map(|entry| entry.value.as_slice())
    }

    /// All entries as `(name, value)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.value.as_slice()))
    }

    /// Number of stored entries (duplicate names count individually).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
