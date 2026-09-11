//! Renderer-process document storage.
//!
//! Trees do not belong to a realm
//! ([ADR 0014](../../../docs/adrs/0014-frames-and-per-frame-realms.md)): the
//! renderer stores every document by its globally unique id, and realms refer
//! to them by id. This mirrors the realm-agnostic C++ DOM layer of Blink and
//! Gecko; only the JS wrappers are realm-associated.

use std::collections::HashMap;

use crate::Parsed;

/// Every document tree the renderer process holds, keyed by document id.
#[derive(Default)]
pub(crate) struct DocumentStore {
    documents: HashMap<u32, Parsed>,
}

impl DocumentStore {
    /// Stores `parsed` and returns its document id.
    pub(crate) fn insert(&mut self, parsed: Parsed) -> u32 {
        let id = parsed.dom.document_id();
        self.documents.insert(id, parsed);
        id
    }

    /// The tree with this document id.
    pub(crate) fn get(&self, id: u32) -> Option<&Parsed> {
        self.documents.get(&id)
    }

    /// Mutable access to the tree with this document id.
    pub(crate) fn get_mut(&mut self, id: u32) -> Option<&mut Parsed> {
        self.documents.get_mut(&id)
    }

    /// Removes and returns the tree with this document id.
    pub(crate) fn remove(&mut self, id: u32) -> Option<Parsed> {
        self.documents.remove(&id)
    }
}
