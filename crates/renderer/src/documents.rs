//! Renderer-process document storage.
//!
//! Trees do not belong to a realm: the renderer stores every document by its
//! globally unique id, and realms refer to them by id. This mirrors the
//! realm-agnostic C++ DOM layer of Blink and Gecko; only the JS wrappers are
//! realm-associated.

use std::collections::{HashMap, HashSet};

use crate::Parsed;
use crate::js::world::JournalEntry;

/// A Blitz document plus the side data our engine keeps per document.
pub(crate) struct BlitzDocument {
    /// The Blitz tree, style, and layout state.
    pub base: blitz_dom::BaseDocument,
    /// Mutations since the last observer drain, in operation order.
    pub journal: Vec<(u64, JournalEntry)>,
    next_journal_position: u64,
    /// Whether any observer watches this document; unobserved mutations are
    /// not recorded.
    recording: bool,
    /// Fragment backings: Blitz has no fragment node kind, so a
    /// `DocumentFragment` is a detached backing element whose children are
    /// the fragment's children. Membership decides the wrapper prototype
    /// and fragment-only algorithms (serialization, insertion).
    fragments: HashSet<blitz_traits::node_id::NodeId>,
}

impl BlitzDocument {
    /// Builds an empty document from `config` (providers ride along).
    pub(crate) fn new(config: blitz_dom::DocumentConfig) -> Self {
        Self {
            base: blitz_dom::BaseDocument::new(config),
            journal: Vec::new(),
            next_journal_position: 1,
            recording: false,
            fragments: HashSet::new(),
        }
    }

    /// Wraps an already-parsed tree.
    pub(crate) fn from_base(base: blitz_dom::BaseDocument) -> Self {
        Self {
            base,
            journal: Vec::new(),
            next_journal_position: 1,
            recording: false,
            fragments: HashSet::new(),
        }
    }

    /// Records `entry` when an observer watches this document.
    pub(crate) fn record(&mut self, entry: JournalEntry) {
        if self.recording {
            let position = self.next_journal_position;
            self.next_journal_position = self.next_journal_position.wrapping_add(1);
            self.journal.push((position, entry));
        }
    }

    /// Enables or disables observer recording for this document.
    pub(crate) fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
    }

    /// Whether `id` backs a `DocumentFragment`.
    pub(crate) fn is_fragment(&self, id: blitz_traits::node_id::NodeId) -> bool {
        self.fragments.contains(&id)
    }

    /// Creates a detached backing element for a new `DocumentFragment`.
    pub(crate) fn create_fragment(&mut self) -> blitz_traits::node_id::NodeId {
        let name = markup5ever::QualName::new(
            None,
            markup5ever::ns!(html),
            markup5ever::LocalName::from("div"),
        );
        let backing = self.base.mutate().create_element(name, Vec::new());
        self.fragments.insert(backing);
        backing
    }
}

/// Every document tree the renderer process holds, keyed by document id.
#[derive(Default)]
pub(crate) struct DocumentStore {
    documents: HashMap<u32, Parsed>,
    next_id: u32,
}

impl DocumentStore {
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut Parsed> {
        self.documents.values_mut()
    }

    /// Stores `parsed` and returns its document id, assigning one when the
    /// parsed document does not carry one yet. Take-and-reinsert cycles keep
    /// their id, so wrappers and observers never lose their document.
    pub(crate) fn insert(&mut self, mut parsed: Parsed) -> u32 {
        if parsed.id == 0 {
            self.next_id = self.next_id.wrapping_add(1);
            parsed.id = self.next_id;
        }
        let id = parsed.id;
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

