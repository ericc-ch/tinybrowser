//! Renderer-process document storage.
//!
//! Trees do not belong to a realm: the renderer stores every document by its
//! globally unique id, and realms refer to them by id. This mirrors the
//! realm-agnostic C++ DOM layer of Blink and Gecko; only the JS wrappers are
//! realm-associated.

use std::collections::{HashMap, HashSet};

use crate::Parsed;
use crate::dom_string::DomString;
use crate::js::world::JournalEntry;

use blitz_dom::NodeData;

/// A node kind Blitz cannot store.
///
/// Blitz's tree has element, text, and comment only. Doctype, processing
/// instruction, and CDATA are real DOM nodes
/// (<https://dom.spec.whatwg.org/#interface-documenttype>,
/// <https://dom.spec.whatwg.org/#interface-processinginstruction>,
/// <https://dom.spec.whatwg.org/#interface-cdatasection>), so each one is a
/// backing node plus this record. A doctype and a processing instruction back
/// onto a comment: neither contributes to a parent's descendant text.
/// CDATA backs onto a text node, because a `CDATASection` is a `Text` node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExtraNode {
    DocumentType {
        name: String,
        public_id: String,
        system_id: String,
    },
    ProcessingInstruction {
        target: String,
        /// Ordered attribute map. Parsing failures store an empty map; later
        /// `setAttribute` writes the map directly and reserializes without
        /// reparsing, so a name the pseudo-attribute grammar rejects (such
        /// as `$`) still round-trips
        /// (<https://dom.spec.whatwg.org/#update-data-from-attributes>).
        attributes: Vec<(String, String)>,
    },
    CDataSection,
}

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
    /// Doctype, processing-instruction, and CDATA records keyed by the
    /// backing node's id. The id's slot version dies with the node, so a
    /// stale entry cannot be mistaken for a new node in the same slot.
    extras: HashMap<blitz_traits::node_id::NodeId, ExtraNode>,
    /// Exact `CharacterData` when the sequence has unpaired surrogates.
    ///
    /// Blitz stores UTF-8 only. [Replace data] and the other `CharacterData`
    /// algorithms operate on UTF-16 code units
    /// (<https://dom.spec.whatwg.org/#concept-cd-replace>), so this map
    /// holds the code units while the backing node keeps the lossy UTF-8
    /// form for layout and serialization.
    utf16_data: HashMap<blitz_traits::node_id::NodeId, DomString>,
    /// Script elements whose [already started] flag is set
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#already-started>).
    started_scripts: HashSet<blitz_traits::node_id::NodeId>,
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
            extras: HashMap::new(),
            utf16_data: HashMap::new(),
            started_scripts: HashSet::new(),
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
            extras: HashMap::new(),
            utf16_data: HashMap::new(),
            started_scripts: HashSet::new(),
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

    /// The side-table record for `id`, when it is a doctype, processing
    /// instruction, or CDATA section.
    pub(crate) fn extra(&self, id: blitz_traits::node_id::NodeId) -> Option<&ExtraNode> {
        self.extras.get(&id)
    }

    /// Whether `id` is a document type node.
    pub(crate) fn is_doctype(&self, id: blitz_traits::node_id::NodeId) -> bool {
        matches!(self.extras.get(&id), Some(ExtraNode::DocumentType { .. }))
    }

    /// Whether `id` is a processing instruction.
    pub(crate) fn is_processing_instruction(&self, id: blitz_traits::node_id::NodeId) -> bool {
        matches!(
            self.extras.get(&id),
            Some(ExtraNode::ProcessingInstruction { .. })
        )
    }

    /// Whether `id` is a CDATA section.
    pub(crate) fn is_cdata(&self, id: blitz_traits::node_id::NodeId) -> bool {
        matches!(self.extras.get(&id), Some(ExtraNode::CDataSection))
    }

    /// Creates a document type whose node document is this document.
    ///
    /// The backing comment stays empty: a doctype's node value is null and
    /// its data is the name, public id, and system id
    /// (<https://dom.spec.whatwg.org/#dom-domimplementation-createdocumenttype>).
    pub(crate) fn create_doctype(
        &mut self,
        name: String,
        public_id: String,
        system_id: String,
    ) -> blitz_traits::node_id::NodeId {
        let backing = self.base.mutate().create_comment_node("");
        self.extras.insert(
            backing,
            ExtraNode::DocumentType {
                name,
                public_id,
                system_id,
            },
        );
        backing
    }

    /// Inserts a doctype as document child `index`, or at the end when the
    /// parsed tree has fewer children.
    ///
    /// The HTML and XML parsers drop the doctype token. The caller recovered
    /// it from the source preamble and passes the number of document children
    /// that precede it.
    pub(crate) fn insert_doctype_child(
        &mut self,
        name: String,
        public_id: String,
        system_id: String,
        index: usize,
    ) {
        let id = self.create_doctype(name, public_id, system_id);
        let root = self.base.root_node().id;
        let anchor = self
            .base
            .get_node(root)
            .and_then(|root| root.children.get(index).copied());
        if let Some(anchor) = anchor {
            self.base.mutate().insert_nodes_before(anchor, &[id]);
        } else {
            self.base.mutate().append_children(root, &[id]);
        }
    }

    /// Creates a processing instruction
    /// (<https://dom.spec.whatwg.org/#create-a-processing-instruction-node>).
    ///
    /// The backing comment holds the data, so character-data operations and
    /// parent text content keep the comment's rules.
    pub(crate) fn create_processing_instruction(
        &mut self,
        target: String,
        data: &str,
    ) -> blitz_traits::node_id::NodeId {
        let backing = self.base.mutate().create_comment_node(data);
        // Initialize updates the attribute map from `data`. A parse error
        // leaves the map empty
        // (<https://dom.spec.whatwg.org/#processinginstruction-initialize>).
        let attributes =
            crate::pseudo_attributes::parse_pseudo_attributes(data).unwrap_or_default();
        self.extras.insert(
            backing,
            ExtraNode::ProcessingInstruction { target, attributes },
        );
        backing
    }

    /// Replaces the ordered attribute map of a processing instruction.
    ///
    /// Callers that already updated the map from a `setAttribute`-style
    /// mutation pass the new map and then replace the data with
    /// `piAttributesAlreadyUpdated` set, so this write is not parsed again
    /// (<https://dom.spec.whatwg.org/#concept-cd-replace>).
    pub(crate) fn set_pi_attributes(
        &mut self,
        id: blitz_traits::node_id::NodeId,
        attributes: Vec<(String, String)>,
    ) {
        if let Some(ExtraNode::ProcessingInstruction {
            attributes: stored, ..
        }) = self.extras.get_mut(&id)
        {
            *stored = attributes;
        }
    }

    /// The ordered attribute map of a processing instruction.
    pub(crate) fn pi_attributes(
        &self,
        id: blitz_traits::node_id::NodeId,
    ) -> Option<&[(String, String)]> {
        match self.extras.get(&id) {
            Some(ExtraNode::ProcessingInstruction { attributes, .. }) => Some(attributes),
            _ => None,
        }
    }

    /// Creates a CDATA section
    /// (<https://dom.spec.whatwg.org/#create-a-cdata-section-node>).
    ///
    /// The backing text node is the section's data, so `Text` operations
    /// (`splitText`, `wholeText`, descendant text) apply.
    pub(crate) fn create_cdata_section(&mut self, data: &str) -> blitz_traits::node_id::NodeId {
        let backing = self.base.mutate().create_text_node(data);
        self.extras.insert(backing, ExtraNode::CDataSection);
        backing
    }

    /// Creates a text node whose data is `data`
    /// (<https://dom.spec.whatwg.org/#create-a-text-node>).
    pub(crate) fn create_text(&mut self, data: &DomString) -> blitz_traits::node_id::NodeId {
        let backing = self.base.mutate().create_text_node(&data.to_string_lossy());
        self.set_exact_character_data(backing, data);
        backing
    }

    /// Creates a comment whose data is `data`
    /// (<https://dom.spec.whatwg.org/#create-a-comment-node>).
    pub(crate) fn create_comment(&mut self, data: &DomString) -> blitz_traits::node_id::NodeId {
        let backing = self
            .base
            .mutate()
            .create_comment_node(&data.to_string_lossy());
        self.set_exact_character_data(backing, data);
        backing
    }

    /// The `CharacterData` of `id` as UTF-16 code units
    /// (<https://dom.spec.whatwg.org/#characterdata>).
    pub(crate) fn character_data(&self, id: blitz_traits::node_id::NodeId) -> DomString {
        if let Some(stored) = self.utf16_data.get(&id) {
            return stored.clone();
        }
        match self.base.get_node(id).map(|node| &node.data) {
            Some(NodeData::Text(text)) => DomString::from(text.content.clone()),
            Some(NodeData::Comment { contents }) => DomString::from(contents.clone()),
            _ => DomString::default(),
        }
    }

    /// Records exact code units when they cannot live in the UTF-8 tree.
    pub(crate) fn set_exact_character_data(
        &mut self,
        id: blitz_traits::node_id::NodeId,
        data: &DomString,
    ) {
        if data.has_unpaired_surrogate() {
            self.utf16_data.insert(id, data.clone());
        } else {
            self.utf16_data.remove(&id);
        }
    }

    /// Whether `id` has already started as a script
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#already-started>).
    pub(crate) fn script_already_started(&self, id: blitz_traits::node_id::NodeId) -> bool {
        self.started_scripts.contains(&id)
    }

    /// Sets the already-started flag. Returns whether this was the first set.
    pub(crate) fn mark_script_started(&mut self, id: blitz_traits::node_id::NodeId) -> bool {
        self.started_scripts.insert(id)
    }

    /// Copies the side-table record of `from` onto `to`, then walks both
    /// subtrees in lockstep. `deep_clone_node` copies Blitz data only.
    pub(crate) fn copy_extra_subtree(
        &mut self,
        from: blitz_traits::node_id::NodeId,
        to: blitz_traits::node_id::NodeId,
    ) {
        if let Some(extra) = self.extras.get(&from).cloned() {
            self.extras.insert(to, extra);
        }
        if let Some(data) = self.utf16_data.get(&from).cloned() {
            self.utf16_data.insert(to, data);
        }
        let from_children: Vec<blitz_traits::node_id::NodeId> = self
            .base
            .get_node(from)
            .map(|node| node.children.iter().copied().collect())
            .unwrap_or_default();
        let to_children: Vec<blitz_traits::node_id::NodeId> = self
            .base
            .get_node(to)
            .map(|node| node.children.iter().copied().collect())
            .unwrap_or_default();
        for (from_child, to_child) in from_children.into_iter().zip(to_children) {
            self.copy_extra_subtree(from_child, to_child);
        }
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
