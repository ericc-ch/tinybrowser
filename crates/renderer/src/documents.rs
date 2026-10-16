//! Renderer-process document storage.
//!
//! Trees do not belong to a realm: the renderer stores every document by its
//! globally unique id, and realms refer to them by id. This mirrors the
//! realm-agnostic C++ DOM layer of Blink and Gecko; only the JS wrappers are
//! realm-associated.

use std::collections::{HashMap, HashSet};

use crate::Parsed;
use crate::dom::tree::NodeId;
use crate::dom_string::DomString;

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
    /// Fragment backings: detached backing elements (for `createDocumentFragment`)
    /// plus real `Fragment` nodes in the Blitz tree (such as template
    /// contents). Membership decides the wrapper prototype and fragment-only
    /// algorithms (serialization, insertion).
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
    /// Live ranges by slot, adjusted by every structural mutation. Slots
    /// free with the document; see `dom::range::RangeTable`.
    pub ranges: crate::dom::range::RangeTable,
    /// Character data that contains an unpaired surrogate. The UTF-8 tree
    /// stores the lossy form for layout; this map is what DOM reads return
    /// (<https://infra.spec.whatwg.org/#javascript-string-convert>).
    exact_text: HashMap<blitz_traits::node_id::NodeId, DomString>,
    /// Attribute values that contain an unpaired surrogate. The tree attribute
    /// drops those units so a CSS `id` selector stays distinct from U+FFFD.
    exact_attrs: HashMap<(blitz_traits::node_id::NodeId, String), DomString>,
    /// Template element -> `(inert document id, contents fragment)`.
    /// The fragment's node document is not the host
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#template-contents>).
    template_owners: HashMap<blitz_traits::node_id::NodeId, (u32, blitz_traits::node_id::NodeId)>,
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
            ranges: crate::dom::range::RangeTable::default(),
            exact_text: HashMap::new(),
            exact_attrs: HashMap::new(),
            template_owners: HashMap::new(),
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
            ranges: crate::dom::range::RangeTable::default(),
            exact_text: HashMap::new(),
            exact_attrs: HashMap::new(),
            template_owners: HashMap::new(),
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

    /// Whether `id` backs a `DocumentFragment`: a flagged backing element
    /// or a real `Fragment` node (such as template contents).
    pub(crate) fn is_fragment(&self, id: blitz_traits::node_id::NodeId) -> bool {
        self.fragments.contains(&id)
            || self.base.get_node(id).is_some_and(|node| {
                matches!(
                    node.data,
                    NodeData::Fragment { .. } | NodeData::ShadowRoot { .. }
                )
            })
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
        name: &str,
        public_id: &str,
        system_id: &str,
    ) -> blitz_traits::node_id::NodeId {
        self.base
            .mutate()
            .create_doctype_node(name, public_id, system_id)
    }

    /// Creates a processing instruction
    /// (<https://dom.spec.whatwg.org/#create-a-processing-instruction-node>).
    ///
    /// The tree holds the data; the attribute map is initialized from it. A
    /// parse error leaves the map empty
    /// (<https://dom.spec.whatwg.org/#processinginstruction-initialize>).
    pub(crate) fn create_processing_instruction(
        &mut self,
        target: &str,
        data: &str,
    ) -> blitz_traits::node_id::NodeId {
        let id = self
            .base
            .mutate()
            .create_processing_instruction_node(target, data);
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
        // Plain insert: both callers hold a PI (creation and the guarded
        // reparse path), and silently dropping a write desyncs the map from
        // the serialized data it produces.
        self.pi_attributes.insert(id, attributes);
    }

    /// Creates a CDATA section whose data is `data`
    /// (<https://dom.spec.whatwg.org/#dom-document-createcdatasection>).
    pub(crate) fn create_cdata_section(&mut self, data: &str) -> blitz_traits::node_id::NodeId {
        self.base.mutate().create_cdata_section_node(data)
    }

    /// Copies the PI attribute map of `from` onto `to`, then walks both
    /// subtrees in lockstep. `deep_clone_node` copies Blitz data only.
    pub(crate) fn copy_pi_subtree(
        &mut self,
        from: blitz_traits::node_id::NodeId,
        to: blitz_traits::node_id::NodeId,
    ) {
        // Pure tax when the document never saw a PI: skip the walk entirely.
        if self.pi_attributes.is_empty() {
            return;
        }
        let mut stack = vec![(from, to)];
        while let Some((from, to)) = stack.pop() {
            if let Some(attributes) = self.pi_attributes.get(&from).cloned() {
                self.pi_attributes.insert(to, attributes);
            }
            stack.extend(crate::dom::adoption::lockstep_children(
                &self.base, from, &self.base, to,
            ));
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
        let id = self.base.mutate().create_text_node(&data.to_string_lossy());
        self.remember_character_data(id, data);
        id
    }

    /// Creates a comment whose data is `data`
    /// (<https://dom.spec.whatwg.org/#create-a-comment-node>).
    pub(crate) fn create_comment(&mut self, data: &DomString) -> blitz_traits::node_id::NodeId {
        let id = self
            .base
            .mutate()
            .create_comment_node(&data.to_string_lossy());
        self.remember_character_data(id, data);
        id
    }

    /// Keeps `data` when it contains an unpaired surrogate. The tree itself
    /// stores the lossy UTF-8 form.
    pub(crate) fn remember_character_data(
        &mut self,
        id: blitz_traits::node_id::NodeId,
        data: &DomString,
    ) {
        if data.has_unpaired_surrogate() {
            self.exact_text.insert(id, data.clone());
        } else {
            self.exact_text.remove(&id);
        }
    }

    /// Keeps an attribute value that contains an unpaired surrogate. The tree
    /// attribute is the form with those units dropped.
    pub(crate) fn remember_attribute(
        &mut self,
        id: blitz_traits::node_id::NodeId,
        name: &str,
        value: DomString,
    ) {
        let key = (id, name.to_owned());
        if value.has_unpaired_surrogate() {
            self.exact_attrs.insert(key, value);
        } else {
            self.exact_attrs.remove(&key);
        }
    }

    /// The exact attribute value when it contains an unpaired surrogate.
    pub(crate) fn attribute_units(
        &self,
        id: blitz_traits::node_id::NodeId,
        name: &str,
    ) -> Option<&DomString> {
        self.exact_attrs.get(&(id, name.to_owned()))
    }

    /// Records the inert document that owns a template's contents.
    pub(crate) fn set_template_owner(
        &mut self,
        template: blitz_traits::node_id::NodeId,
        document: u32,
        fragment: blitz_traits::node_id::NodeId,
    ) {
        self.template_owners.insert(template, (document, fragment));
    }

    /// The inert document and fragment for a template, when migrated.
    pub(crate) fn template_owner(
        &self,
        template: blitz_traits::node_id::NodeId,
    ) -> Option<(u32, blitz_traits::node_id::NodeId)> {
        self.template_owners.get(&template).copied()
    }

    /// The `CharacterData` of `id` as UTF-16 code units
    /// (<https://dom.spec.whatwg.org/#characterdata>).
    ///
    /// Unpaired surrogates live in `exact_text`. The UTF-8 tree holds the
    /// lossy form for layout.
    pub(crate) fn character_data(&self, id: blitz_traits::node_id::NodeId) -> DomString {
        if let Some(exact) = self.exact_text.get(&id) {
            return exact.clone();
        }
        match self.base.get_node(id).map(|node| &node.data) {
            Some(NodeData::Text(text)) => DomString::from(text.content.clone()),
            Some(
                NodeData::Comment { contents }
                | NodeData::ProcessingInstruction { contents, .. }
                | NodeData::CDataSection { contents },
            ) => DomString::from(contents.clone()),
            _ => DomString::default(),
        }
    }

    /// Descendant text content, keeping unpaired surrogates
    /// (<https://dom.spec.whatwg.org/#concept-descendant-text-content>).
    pub(crate) fn descendant_dom_text(&self, id: blitz_traits::node_id::NodeId) -> DomString {
        let mut text = DomString::default();
        let mut stack = self
            .base
            .get_node(id)
            .map(|node| node.children.iter().rev().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        while let Some(current) = stack.pop() {
            let Some(node) = self.base.get_node(current) else {
                continue;
            };
            match &node.data {
                NodeData::Text(_) | NodeData::CDataSection { .. } => {
                    text.push_dom(&self.character_data(current));
                }
                NodeData::Element(_) => {
                    stack.extend(node.children.iter().rev().copied());
                }
                _ => {}
            }
        }
        text
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

    /// Copies the already-started flag of `from` onto `to`, then walks both
    /// subtrees in lockstep. Cloning sets the copy's flag from the original
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#script-processing-model:cloning-steps>).
    pub(crate) fn copy_script_started(
        &mut self,
        from: blitz_traits::node_id::NodeId,
        to: blitz_traits::node_id::NodeId,
    ) {
        // Pure tax when no script ever started: skip the walk entirely.
        if self.started_scripts.is_empty() {
            return;
        }
        let mut stack = vec![(from, to)];
        while let Some((from, to)) = stack.pop() {
            if self.started_scripts.contains(&from) {
                self.started_scripts.insert(to);
            }
            stack.extend(crate::dom::adoption::lockstep_children(
                &self.base, from, &self.base, to,
            ));
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

/// One recorded tree mutation, in observer-delivery shape.
///
/// Binding and parser call sites push these onto the owning document's
/// journal; Blitz only tracks a coarse changed set, so the delivery payload
/// lives here.
#[derive(Clone, Debug)]
pub(crate) enum JournalEntry {
    ChildList {
        target: NodeId,
        added: Vec<NodeId>,
        removed: Vec<NodeId>,
        previous: Option<NodeId>,
        next: Option<NodeId>,
    },
    Attributes {
        target: NodeId,
        name: String,
        namespace: String,
        old_value: Option<String>,
    },
    CharacterData {
        target: NodeId,
        old_value: DomString,
    },
}
