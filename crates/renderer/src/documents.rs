//! Renderer-process document storage.
//!
//! Trees do not belong to a realm: the renderer stores every document by its
//! globally unique id, and realms refer to them by id. This mirrors the
//! realm-agnostic C++ DOM layer of Blink and Gecko; only the JS wrappers are
//! realm-associated.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::Parsed;
use crate::js::world::JournalEntry;

/// Process-wide mutation order for observer delivery across documents.
static NEXT_JOURNAL_POSITION: AtomicU64 = AtomicU64::new(1);

/// A node kind Blitz does not model, backed by a comment node plus this record.
///
/// Backings live in the tree like any node, so parent/child walks, style,
/// and layout need no changes; only the JS-visible surface (interface brand,
/// `nodeType`, `nodeName`, character data, serialization, insertion
/// validity) consults the table. Blitz ids carry a slot version that bumps
/// on reuse, so a stale entry can never alias a new node in a recycled
/// slot; entries for removed nodes are simply never read again.
#[derive(Clone, Debug)]
pub(crate) enum SyntheticKind {
    /// A processing instruction: backing comment holds `target + ' ' + data`.
    Pi {
        target: String,
    },
    /// A CDATA section: backing comment holds the data.
    CData,
    /// A doctype: fields live here; the backing holds nothing.
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
}

/// A Blitz document plus the side data our engine keeps per document.
pub(crate) struct BlitzDocument {
    /// The Blitz tree, style, and layout state.
    pub base: blitz_dom::BaseDocument,
    /// Mutations since the last observer drain, in operation order.
    pub journal: Vec<(u64, JournalEntry)>,
    /// Whether any observer watches this document; unobserved mutations are
    /// not recorded.
    recording: bool,
    /// Fragment backings: Blitz has no fragment node kind, so a
    /// `DocumentFragment` is a detached backing element whose children are
    /// the fragment's children. Membership decides the wrapper prototype
    /// and fragment-only algorithms (serialization, insertion).
    fragments: HashSet<blitz_traits::node_id::NodeId>,
    /// Processing instructions, CDATA sections, and doctypes by backing id.
    synthetic: HashMap<blitz_traits::node_id::NodeId, SyntheticKind>,
}

impl BlitzDocument {
    /// Builds an empty document from `config` (providers ride along).
    pub(crate) fn new(config: blitz_dom::DocumentConfig) -> Self {
        Self {
            base: blitz_dom::BaseDocument::new(config),
            journal: Vec::new(),
            recording: false,
            fragments: HashSet::new(),
            synthetic: HashMap::new(),
        }
    }

    /// Wraps an already-parsed tree.
    pub(crate) fn from_base(base: blitz_dom::BaseDocument) -> Self {
        Self {
            base,
            journal: Vec::new(),
            recording: false,
            fragments: HashSet::new(),
            synthetic: HashMap::new(),
        }
    }

    /// Records `entry` when an observer watches this document.
    pub(crate) fn record(&mut self, entry: JournalEntry) {
        if self.recording {
            let position = NEXT_JOURNAL_POSITION.fetch_add(1, Ordering::SeqCst);
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

    /// The synthetic kind of a backing node, if it stands in for a
    /// processing instruction, CDATA section, or doctype.
    pub(crate) fn synthetic_kind(
        &self,
        id: blitz_traits::node_id::NodeId,
    ) -> Option<&SyntheticKind> {
        self.synthetic.get(&id)
    }

    /// Creates a processing-instruction node: a backing comment holding
    /// `target + ' ' + data` (or just `target` when data is empty).
    pub(crate) fn create_pi(&mut self, target: &str, data: &str) -> blitz_traits::node_id::NodeId {
        let contents = if data.is_empty() {
            target.to_owned()
        } else {
            format!("{target} {data}")
        };
        let backing = self.base.mutate().create_comment_node(&contents);
        self.synthetic.insert(
            backing,
            SyntheticKind::Pi {
                target: target.to_owned(),
            },
        );
        backing
    }

    /// Creates a CDATA-section node: a backing comment holding the data.
    pub(crate) fn create_cdata(&mut self, data: &str) -> blitz_traits::node_id::NodeId {
        let backing = self.base.mutate().create_comment_node(data);
        self.synthetic.insert(backing, SyntheticKind::CData);
        backing
    }

    /// Creates a doctype node: a backing comment plus the declaration fields.
    pub(crate) fn create_doctype(
        &mut self,
        name: &str,
        public_id: &str,
        system_id: &str,
    ) -> blitz_traits::node_id::NodeId {
        let backing = self.base.mutate().create_comment_node("");
        self.synthetic.insert(
            backing,
            SyntheticKind::Doctype {
                name: name.to_owned(),
                public_id: public_id.to_owned(),
                system_id: system_id.to_owned(),
            },
        );
        backing
    }

    /// Copies a synthetic record onto a freshly cloned backing (same-document
    /// `cloneNode` clones the backing through Blitz, which knows nothing of
    /// the record).
    pub(crate) fn clone_synthetic(
        &mut self,
        from: blitz_traits::node_id::NodeId,
        to: blitz_traits::node_id::NodeId,
    ) {
        if let Some(kind) = self.synthetic.get(&from).cloned() {
            self.synthetic.insert(to, kind);
        }
    }

    /// The document's doctype backing, when one was created or captured.
    pub(crate) fn document_doctype(&self) -> Option<blitz_traits::node_id::NodeId> {
        let root = self.base.root_node().id;
        self.base.get_node(root).and_then(|root| {
            root.children.iter().copied().find(|child| {
                matches!(
                    self.synthetic.get(child),
                    Some(SyntheticKind::Doctype { .. })
                )
            })
        })
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
