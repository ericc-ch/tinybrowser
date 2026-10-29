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
    /// Every per-document side table, all default-constructed.
    pub side: SideTables,
}

/// Side data that is not part of the Blitz tree.
#[derive(Default)]
pub struct SideTables {
    /// Mutations since the last observer drain, in operation order.
    pub journal: Vec<(u64, JournalEntry)>,
    next_journal_position: u64,
    /// Whether any observer watches this document; unobserved mutations are
    /// not recorded.
    recording: bool,
    /// Script elements whose [already started] flag is set
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#already-started>).
    started_scripts: HashSet<blitz_traits::node_id::NodeId>,
    /// Selects whose option selectedness was materialized from attributes and
    /// the reset rule. Insertions, removals, and the relevant attribute
    /// changes invalidate the entry
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#selectedness-setting-algorithm>).
    selectedness_normalized: HashSet<blitz_traits::node_id::NodeId>,
    /// Stored option selectedness. Blitz folds selectedness into the
    /// `selected` attribute unless a dirty bit is set, but the spec keeps a
    /// separate selectedness slot and dirtiness flag
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-selectedness>).
    option_selectedness: HashMap<blitz_traits::node_id::NodeId, bool>,
    /// Options whose selectedness was set by script or a selectedIndex/value
    /// setter. A later `selected` attribute change does not override them
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-dirtiness>).
    option_dirtiness: HashSet<blitz_traits::node_id::NodeId>,
    /// Inputs whose stored value is the sanitized current value while the
    /// element is still logically pristine. The dirty slot also drives Blitz's
    /// layout editor, so a marker keeps the sanitized text on screen without
    /// making the value user-dirty
    /// (<https://html.spec.whatwg.org/multipage/input.html#concept-fe-value>).
    pristine_values: HashSet<blitz_traits::node_id::NodeId>,
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
        Self::from_base(blitz_dom::BaseDocument::new(config))
    }

    /// Wraps an already-parsed tree.
    pub(crate) fn from_base(base: blitz_dom::BaseDocument) -> Self {
        Self {
            base,
            side: SideTables::default(),
        }
    }

    /// Whether a select's option selectedness is already materialized.
    pub(crate) fn selectedness_normalized(&self, select: blitz_traits::node_id::NodeId) -> bool {
        self.side.selectedness_normalized.contains(&select)
    }

    /// Marks a select's option selectedness as materialized.
    pub(crate) fn mark_selectedness_normalized(&mut self, select: blitz_traits::node_id::NodeId) {
        self.side.selectedness_normalized.insert(select);
    }

    /// Drops the materialized selectedness for one select.
    pub(crate) fn invalidate_selectedness(&mut self, select: blitz_traits::node_id::NodeId) {
        self.side.selectedness_normalized.remove(&select);
    }

    /// The stored selectedness slot for an option, when materialized.
    pub(crate) fn option_selectedness(
        &self,
        option: blitz_traits::node_id::NodeId,
    ) -> Option<bool> {
        self.side.option_selectedness.get(&option).copied()
    }

    /// Stores an option's selectedness slot.
    pub(crate) fn set_option_selectedness(
        &mut self,
        option: blitz_traits::node_id::NodeId,
        selected: bool,
    ) {
        self.side.option_selectedness.insert(option, selected);
    }

    /// Whether an option's selectedness was set by script.
    pub(crate) fn option_dirty(&self, option: blitz_traits::node_id::NodeId) -> bool {
        self.side.option_dirtiness.contains(&option)
    }

    /// Marks an option's selectedness as script-set.
    pub(crate) fn mark_option_dirty(&mut self, option: blitz_traits::node_id::NodeId) {
        self.side.option_dirtiness.insert(option);
    }

    /// Clears an option's dirtiness, so its `selected` attribute applies
    /// again.
    pub(crate) fn clear_option_dirty(&mut self, option: blitz_traits::node_id::NodeId) {
        self.side.option_dirtiness.remove(&option);
    }

    /// Whether an input's stored value is a sanitized pristine value.
    pub(crate) fn pristine_value(&self, node: blitz_traits::node_id::NodeId) -> bool {
        self.side.pristine_values.contains(&node)
    }

    /// Marks an input's stored value as sanitized but pristine.
    pub(crate) fn mark_pristine_value(&mut self, node: blitz_traits::node_id::NodeId) {
        self.side.pristine_values.insert(node);
    }

    /// Clears the pristine marker before a user value supersedes it.
    pub(crate) fn clear_pristine_value(&mut self, node: blitz_traits::node_id::NodeId) {
        self.side.pristine_values.remove(&node);
    }

    /// Records `entry` when an observer watches this document.
    pub(crate) fn record(&mut self, entry: JournalEntry) {
        if self.side.recording {
            let position = self.side.next_journal_position;
            self.side.next_journal_position = self.side.next_journal_position.wrapping_add(1);
            self.side.journal.push((position, entry));
        }
    }

    /// Enables or disables observer recording for this document.
    pub(crate) fn set_recording(&mut self, recording: bool) {
        self.side.recording = recording;
    }

    /// Whether `id` is a `DocumentFragment`: a fragment node or a shadow
    /// root, which is one too
    /// (<https://dom.spec.whatwg.org/#interface-documentfragment>,
    /// <https://dom.spec.whatwg.org/#interface-shadowroot>).
    pub(crate) fn is_fragment(&self, id: blitz_traits::node_id::NodeId) -> bool {
        self.base.get_node(id).is_some_and(|node| {
            matches!(
                node.data,
                NodeData::Fragment { .. } | NodeData::ShadowRoot { .. }
            )
        })
    }

    /// Creates a detached `DocumentFragment`
    /// (<https://dom.spec.whatwg.org/#dom-document-createdocumentfragment>).
    pub(crate) fn create_fragment(&mut self) -> blitz_traits::node_id::NodeId {
        self.base.mutate().create_fragment_node()
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
    pub(crate) fn create_processing_instruction(
        &mut self,
        target: &str,
        data: &str,
    ) -> blitz_traits::node_id::NodeId {
        self.base
            .mutate()
            .create_processing_instruction_node(target, data)
    }

    /// Creates a CDATA section whose data is `data`
    /// (<https://dom.spec.whatwg.org/#dom-document-createcdatasection>).
    pub(crate) fn create_cdata_section(&mut self, data: &str) -> blitz_traits::node_id::NodeId {
        self.base.mutate().create_cdata_section_node(data)
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
            self.side.exact_text.insert(id, data.clone());
        } else {
            self.side.exact_text.remove(&id);
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
            self.side.exact_attrs.insert(key, value);
        } else {
            self.side.exact_attrs.remove(&key);
        }
    }

    /// The exact attribute value when it contains an unpaired surrogate.
    pub(crate) fn attribute_units(
        &self,
        id: blitz_traits::node_id::NodeId,
        name: &str,
    ) -> Option<&DomString> {
        self.side.exact_attrs.get(&(id, name.to_owned()))
    }

    /// Records the inert document that owns a template's contents.
    pub(crate) fn set_template_owner(
        &mut self,
        template: blitz_traits::node_id::NodeId,
        document: u32,
        fragment: blitz_traits::node_id::NodeId,
    ) {
        self.side
            .template_owners
            .insert(template, (document, fragment));
    }

    /// The inert document and fragment for a template, when migrated.
    pub(crate) fn template_owner(
        &self,
        template: blitz_traits::node_id::NodeId,
    ) -> Option<(u32, blitz_traits::node_id::NodeId)> {
        self.side.template_owners.get(&template).copied()
    }

    /// The `CharacterData` of `id` as UTF-16 code units
    /// (<https://dom.spec.whatwg.org/#characterdata>).
    ///
    /// Unpaired surrogates live in `exact_text`. The UTF-8 tree holds the
    /// lossy form for layout.
    pub(crate) fn character_data(&self, id: blitz_traits::node_id::NodeId) -> DomString {
        if let Some(exact) = self.side.exact_text.get(&id) {
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
        self.side.started_scripts.contains(&id)
    }

    /// Sets the already-started flag. Returns whether this was the first set.
    pub(crate) fn mark_script_started(&mut self, id: blitz_traits::node_id::NodeId) -> bool {
        self.side.started_scripts.insert(id)
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
        if self.side.started_scripts.is_empty() {
            return;
        }
        let mut stack = vec![(from, to)];
        while let Some((from, to)) = stack.pop() {
            if self.side.started_scripts.contains(&from) {
                self.side.started_scripts.insert(to);
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
