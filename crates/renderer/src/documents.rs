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
    /// Processing-instruction attribute maps, keyed by node id. The id's
    /// slot version dies with the node, so a stale entry cannot be mistaken
    /// for a new node in the same slot. The tree holds the data; this map
    /// holds the parsed pseudo-attributes
    /// (<https://dom.spec.whatwg.org/#processinginstruction-initialize>).
    /// Everything else about PI, doctype, and CDATA nodes lives in the
    /// Blitz tree as real node kinds.
    pi_attributes: HashMap<blitz_traits::node_id::NodeId, Vec<(String, String)>>,
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
            pi_attributes: HashMap::new(),
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
            pi_attributes: HashMap::new(),
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

    /// Creates a document type whose node document is this document
    /// (<https://dom.spec.whatwg.org/#dom-domimplementation-createdocumenttype>).
    pub(crate) fn create_doctype(
        &mut self,
        name: String,
        public_id: String,
        system_id: String,
    ) -> blitz_traits::node_id::NodeId {
        self.base
            .mutate()
            .create_doctype_node(&name, &public_id, &system_id)
    }

    /// Creates a processing instruction
    /// (<https://dom.spec.whatwg.org/#create-a-processing-instruction-node>).
    ///
    /// The tree holds the data; the attribute map is initialized from it. A
    /// parse error leaves the map empty
    /// (<https://dom.spec.whatwg.org/#processinginstruction-initialize>).
    pub(crate) fn create_processing_instruction(
        &mut self,
        target: String,
        data: &str,
    ) -> blitz_traits::node_id::NodeId {
        let id = self
            .base
            .mutate()
            .create_processing_instruction_node(&target, data);
        let attributes =
            crate::pseudo_attributes::parse_pseudo_attributes(data).unwrap_or_default();
        self.pi_attributes.insert(id, attributes);
        id
    }

    /// The ordered attribute map of a processing instruction, if it has one.
    pub(crate) fn pi_attributes(
        &self,
        id: blitz_traits::node_id::NodeId,
    ) -> Option<&[(String, String)]> {
        self.pi_attributes.get(&id).map(Vec::as_slice)
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
        if let Some(stored) = self.pi_attributes.get_mut(&id) {
            *stored = attributes;
        }
    }

    /// Creates a CDATA section whose data is `data`
    /// (<https://dom.spec.whatwg.org/#dom-document-createcdatasection>).
    pub(crate) fn create_cdata_section(
        &mut self,
        data: &str,
    ) -> blitz_traits::node_id::NodeId {
        self.base.mutate().create_cdata_section_node(data)
    }

    /// Copies the PI attribute map of `from` onto `to`, then walks both
    /// subtrees in lockstep. `deep_clone_node` copies Blitz data only.
    pub(crate) fn copy_pi_subtree(
        &mut self,
        from: blitz_traits::node_id::NodeId,
        to: blitz_traits::node_id::NodeId,
    ) {
        if let Some(attributes) = self.pi_attributes.get(&from).cloned() {
            self.pi_attributes.insert(to, attributes);
        }
        let from_children = self
            .base
            .get_node(from)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        let to_children = self
            .base
            .get_node(to)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        for (from_child, to_child) in from_children.into_iter().zip(to_children) {
            self.copy_pi_subtree(from_child, to_child);
        }
    }

    /// Initializes the attribute maps of every processing instruction in the
    /// tree from its data. Parsed documents arrive with data but no map;
    /// created nodes initialize at creation
    /// (<https://dom.spec.whatwg.org/#processinginstruction-initialize>).
    pub(crate) fn init_parsed_pi_attributes(&mut self) {
        let root = self.base.root_node().id;
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let (is_pi, data, children) = self
                .base
                .get_node(id)
                .map(|node| {
                    (
                        matches!(node.data, NodeData::ProcessingInstruction { .. }),
                        match &node.data {
                            NodeData::ProcessingInstruction { contents, .. } => {
                                Some(contents.clone())
                            }
                            _ => None,
                        },
                        node.children.clone(),
                    )
                })
                .unwrap_or_default();
            if is_pi {
                let attributes = data
                    .as_deref()
                    .and_then(crate::pseudo_attributes::parse_pseudo_attributes)
                    .unwrap_or_default();
                self.pi_attributes.insert(id, attributes);
            }
            stack.extend(children);
        }
    }

    /// Creates a text node whose data is `data`
    /// (<https://dom.spec.whatwg.org/#create-a-text-node>).
    pub(crate) fn create_text(&mut self, data: &DomString) -> blitz_traits::node_id::NodeId {
        self.base.mutate().create_text_node(&data.to_string_lossy())
    }

    /// Creates a comment whose data is `data`
    /// (<https://dom.spec.whatwg.org/#create-a-comment-node>).
    pub(crate) fn create_comment(&mut self, data: &DomString) -> blitz_traits::node_id::NodeId {
        self.base
            .mutate()
            .create_comment_node(&data.to_string_lossy())
    }

    /// The `CharacterData` of `id` as UTF-16 code units
    /// (<https://dom.spec.whatwg.org/#characterdata>).
    ///
    /// Unpaired surrogates do not survive the UTF-8 tree: they read back as
    /// the replacement character. Preserving them is an upstream gap (see
    /// `docs/progress.md`), not something this map reconstructs.
    pub(crate) fn character_data(&self, id: blitz_traits::node_id::NodeId) -> DomString {
        match self.base.get_node(id).map(|node| &node.data) {
            Some(NodeData::Text(text)) => DomString::from(text.content.clone()),
            Some(NodeData::Comment { contents }) => DomString::from(contents.clone()),
            Some(NodeData::ProcessingInstruction { contents, .. }) => {
                DomString::from(contents.clone())
            }
            Some(NodeData::CDataSection { contents }) => DomString::from(contents.clone()),
            _ => DomString::default(),
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
