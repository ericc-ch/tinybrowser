//! The form control model: form ownership, checkedness, radio button groups,
//! selectability, and the reset algorithm.
//!
//! The state lives in `FormState`; the behavior lives in this module's free
//! functions, which read the tree through [`Document`]. Tree mutation
//! primitives call into them at the points the spec defines as phenomena
//! (<https://html.spec.whatwg.org/multipage/forms.html#form-associated-element>).

mod input;
mod select;

use std::collections::{HashMap, HashSet};

use crate::{Document, DomError, NodeId};

pub use input::{
    control_default_value, control_value, element_value, input_selectable, input_type, input_value,
    selection, selection_supported, set_control_default_value, set_control_value, set_element_value,
    set_input_selectable, set_input_value, set_selection, set_textarea_value, textarea_raw_value,
    textarea_value,
};
pub use select::{
    append_blank_options, maybe_clone_option_into_selectedcontent, node_list_owner,
    option_select_owner, option_selected, option_text, option_value, select_options,
    select_selected_index, select_value, set_option_selected_in_select, set_option_selectedness,
    set_select_selected_index, set_select_value,
};
pub(crate) use input::{reset_textarea_selection_if_changed, textarea_value_before_change};
pub(crate) use select::{apply_default_selectedness, inserted_list_owner, option_added_to_select};

#[derive(Debug, Default)]
pub(crate) struct FormState {
    /// A stored raw value means the input or textarea has a dirty value flag
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-fe-dirty>).
    input_values: HashMap<NodeId, String>,
    /// Last normalized input type, used by the type-change steps
    /// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:type-change-state>).
    input_types: HashMap<NodeId, String>,
    /// Dirty checkedness, separate from the `checked` content attribute
    /// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-checked-dirty-flag>).
    checkedness: HashMap<NodeId, bool>,
    /// Input indeterminateness is independent of checkedness
    /// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-indeterminate>).
    indeterminate: HashMap<NodeId, bool>,
    /// Per-option selectedness and its dirty flag
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-selectedness>).
    option_selectedness: HashMap<NodeId, bool>,
    option_dirty_selected: HashSet<NodeId>,
    /// Last selection-support state across input type changes.
    input_selectable: HashMap<NodeId, bool>,
    /// Start, end and direction in UTF-16 code units; absence is (0, 0, none)
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-textarea/input-selection>).
    selections: HashMap<NodeId, (u32, u32, u8)>,
}

impl FormState {
    /// Copy the raw dirty value during node cloning
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-textarea-element:concept-node-clone-ext>).
    pub(crate) fn clone_dirty_value(&mut self, from: NodeId, to: NodeId) {
        if let Some(value) = self.input_values.get(&from).cloned() {
            self.input_values.insert(to, value);
        }
    }

    pub(crate) fn forget(&mut self, id: NodeId) {
        self.input_values.remove(&id);
        self.input_types.remove(&id);
        self.checkedness.remove(&id);
        self.indeterminate.remove(&id);
        self.option_selectedness.remove(&id);
        self.option_dirty_selected.remove(&id);
        self.input_selectable.remove(&id);
        self.selections.remove(&id);
    }
}

/// Run the form-control steps after setting an attribute.
pub(crate) fn attribute_set(document: &mut Document, node: NodeId, name: &str) -> Result<(), DomError> {
    if name.eq_ignore_ascii_case("selected") {
        select::refresh_option_selectedness(document, node);
        if let Some(select) = select::option_select_owner(document, node) {
            select::apply_default_selectedness(document, select);
        }
    }
    if name.eq_ignore_ascii_case("type") {
        input::refresh_input_type(document, node)?;
    }
    if name.eq_ignore_ascii_case("name") || name.eq_ignore_ascii_case("checked") {
        refresh_radio_group(document, node);
    }
    if document.html_local_is(node, "select")
        && (name.eq_ignore_ascii_case("multiple") || name.eq_ignore_ascii_case("size"))
    {
        select::apply_default_selectedness(document, node);
    }
    Ok(())
}

/// Run the form-control steps after removing an attribute.
pub(crate) fn attribute_removed(
    document: &mut Document,
    node: NodeId,
    name: &str,
) -> Result<(), DomError> {
    if name.eq_ignore_ascii_case("selected") {
        select::refresh_option_selectedness(document, node);
        if let Some(select) = select::option_select_owner(document, node) {
            select::apply_default_selectedness(document, select);
        }
    }
    if name.eq_ignore_ascii_case("type") {
        input::refresh_input_type(document, node)?;
    }
    if document.html_local_is(node, "select")
        && (name.eq_ignore_ascii_case("multiple") || name.eq_ignore_ascii_case("size"))
    {
        select::apply_default_selectedness(document, node);
    }
    Ok(())
}

    /// The reset algorithm for a form control: an `input`/`textarea` clears its
    /// dirty value and checkedness flags, and a `select` restores each option's
    /// selectedness from its `selected` attribute before running the
    /// selectedness setting algorithm
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-form-reset-control>).
    pub fn reset_control(document: &mut Document, id: NodeId) {
        if document.html_local_is(id, "input") || document.html_local_is(id, "textarea") {
            document.form.input_values.remove(&id);
            document.form.checkedness.remove(&id);
            document.form.indeterminate.remove(&id);
        } else if document.html_local_is(id, "select") {
            for option in select::select_options(document, id) {
                let selected = document.attribute(option, "selected").is_some();
                document.form.option_selectedness.insert(option, selected);
                document.form.option_dirty_selected.remove(&option);
            }
            select::apply_default_selectedness(document, id);
        }
    }

    /// An `input`'s checkedness: the stored value while the dirty checkedness
    /// flag is set, else whether the `checked` content attribute is present
    /// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-checked>).
    #[must_use]
    pub fn checkedness(document: &Document, id: NodeId) -> bool {
        document.form
            .checkedness
            .get(&id)
            .copied()
            .unwrap_or_else(|| document.attribute(id, "checked").is_some())
    }

    /// Sets an input's checkedness, unchecking the rest of a radio button
    /// group when checking a radio
    /// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-state-(type=radio)>).
    pub fn set_input_checkedness(document: &mut Document, id: NodeId, checked: bool) {
        if checked && input::input_type(document, id).as_deref() == Some("radio") {
            uncheck_radio_group(document, id);
        }
        document.form.checkedness.insert(id, checked);
    }

    /// Unchecks the other radios in `id`'s group. A radio button group is the
    /// radios in the same tree with the same form owner and name
    /// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
    fn uncheck_radio_group(document: &mut Document, id: NodeId) {
        let Some(name) = document.attribute(id, "name") else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let owner = form_owner(document, id);
        let scope = tree_root_of(document, id);
        let others: Vec<NodeId> = document.tree().descendants(scope)
            .filter(|&other| {
                other != id && is_radio_named(document, other, &name) && form_owner(document, other) == owner
            })
            .collect();
        for other in others {
            document.form.checkedness.insert(other, false);
        }
    }

    /// Re-applies a checked radio's group rule after its `name` or form owner
    /// changed or it moved trees
    /// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
    pub(crate) fn refresh_radio_group(document: &mut Document, id: NodeId) {
        if checkedness(document, id) && input::input_type(document, id).as_deref() == Some("radio") {
            uncheck_radio_group(document, id);
        }
    }

    /// The form owner of `node` when it is a checked radio, else `None`; used
    /// to detect a form-owner change on insertion
    /// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
    pub(crate) fn checked_radio_form_owner(document: &Document, node: NodeId) -> Option<NodeId> {
        if document.html_local_is(node, "input")
            && input::input_type(document, node).as_deref() == Some("radio")
            && checkedness(document, node)
        {
            form_owner(document, node)
        } else {
            None
        }
    }

    /// The form owner of a form-associated element: the form named by its
    /// `form` attribute, else its nearest ancestor form
    /// (<https://html.spec.whatwg.org/multipage/forms.html#form-owner>).
    #[must_use]
    pub fn form_owner(document: &Document, id: NodeId) -> Option<NodeId> {
        if let Some(reference) = document.attribute(id, "form")
            && !reference.is_empty()
        {
            let root = tree_root_of(document, id);
            let found = document.tree().descendants(root).find(|&other| {
                document.html_local_is(other, "form")
                    && document.attribute(other, "id").as_deref() == Some(reference.as_str())
            });
            if found.is_some() {
                return found;
            }
        }
        nearest_form_ancestor(document, id)
    }

    /// The checked radio in `id`'s radio button group, if any.
    #[must_use]
    pub fn radio_group_checked(document: &Document, id: NodeId) -> Option<NodeId> {
        let name = document.attribute(id, "name")?;
        if name.is_empty() {
            return None;
        }
        let owner = form_owner(document, id);
        let scope = tree_root_of(document, id);
        document.tree().descendants(scope).find(|&other| {
            other != id
                && is_radio_named(document, other, &name)
                && form_owner(document, other) == owner
                && checkedness(document, other)
        })
    }

    /// The nearest ancestor `form` of `node`, if any.
    fn nearest_form_ancestor(document: &Document, node: NodeId) -> Option<NodeId> {
        let mut current = document.parent(node);
        while let Some(parent) = current {
            if document.html_local_is(parent, "form") {
                return Some(parent);
            }
            current = document.parent(parent);
        }
        None
    }

    /// The root of `node`'s tree, for grouping radios outside any form.
    #[must_use]
    pub fn tree_root_of(document: &Document, node: NodeId) -> NodeId {
        let mut current = node;
        while let Some(parent) = document.parent(current) {
            current = parent;
        }
        current
    }

    /// Whether `node` is a radio with the given group name.
    fn is_radio_named(document: &Document, node: NodeId, name: &str) -> bool {
        input::input_type(document, node).as_deref() == Some("radio")
            && document.attribute(node, "name").as_deref() == Some(name)
    }

    /// An `input`'s indeterminateness
    /// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-indeterminate>).
    #[must_use]
    pub fn indeterminate(document: &Document, id: NodeId) -> bool {
        document.form.indeterminate.get(&id).copied().unwrap_or(false)
    }

    /// Sets `id`'s indeterminateness.
    pub fn set_indeterminate(document: &mut Document, id: NodeId, indeterminate: bool) {
        document.form.indeterminate.insert(id, indeterminate);
    }

    /// The input/option cloning steps: propagate value, dirty value, and
    /// checkedness state from `from` to `to`
    /// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:cloning-steps>).
    pub fn clone_form_state(document: &mut Document, from: NodeId, to: NodeId) {
        document.form.clone_dirty_value(from, to);
        if let Some(checked) = document.form.checkedness.get(&from).copied() {
            document.form.checkedness.insert(to, checked);
        }
        if let Some(indeterminate) = document.form.indeterminate.get(&from).copied() {
            document.form.indeterminate.insert(to, indeterminate);
        }
        if let Some(selected) = document.form.option_selectedness.get(&from).copied() {
            document.form.option_selectedness.insert(to, selected);
        }
        if document.form.option_dirty_selected.contains(&from) {
            document.form.option_dirty_selected.insert(to);
        }
    }
