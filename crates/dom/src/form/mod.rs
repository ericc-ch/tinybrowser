//! The form control model: form ownership, checkedness, radio button groups,
//! selectability, and the reset algorithm.
//!
//! The state lives in [`FormState`]. These methods remain on [`Dom`] because
//! they also inspect the tree. Tree mutation primitives call into them at the
//! points the spec defines as phenomena
//! (<https://html.spec.whatwg.org/multipage/forms.html#form-associated-element>).

mod input;
mod select;

use std::collections::{HashMap, HashSet};

use crate::{Dom, DomError, NodeId};

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
pub(crate) fn attribute_set(document: &mut Dom, node: NodeId, name: &str) -> Result<(), DomError> {
    if name.eq_ignore_ascii_case("selected") {
        document.refresh_option_selectedness(node);
        if let Some(select) = document.option_select_owner(node) {
            document.apply_default_selectedness(select);
        }
    }
    if name.eq_ignore_ascii_case("type") {
        document.refresh_input_type(node)?;
    }
    if name.eq_ignore_ascii_case("name") || name.eq_ignore_ascii_case("checked") {
        document.refresh_radio_group(node);
    }
    if document.html_local_is(node, "select")
        && (name.eq_ignore_ascii_case("multiple") || name.eq_ignore_ascii_case("size"))
    {
        document.apply_default_selectedness(node);
    }
    Ok(())
}

/// Run the form-control steps after removing an attribute.
pub(crate) fn attribute_removed(
    document: &mut Dom,
    node: NodeId,
    name: &str,
) -> Result<(), DomError> {
    if name.eq_ignore_ascii_case("selected") {
        document.refresh_option_selectedness(node);
        if let Some(select) = document.option_select_owner(node) {
            document.apply_default_selectedness(select);
        }
    }
    if name.eq_ignore_ascii_case("type") {
        document.refresh_input_type(node)?;
    }
    if document.html_local_is(node, "select")
        && (name.eq_ignore_ascii_case("multiple") || name.eq_ignore_ascii_case("size"))
    {
        document.apply_default_selectedness(node);
    }
    Ok(())
}

impl Dom {
    /// The reset algorithm for a form control: an `input`/`textarea` clears its
    /// dirty value and checkedness flags, and a `select` restores each option's
    /// selectedness from its `selected` attribute before running the
    /// selectedness setting algorithm
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-form-reset-control>).
    pub fn reset_control(&mut self, id: NodeId) {
        if self.html_local_is(id, "input") || self.html_local_is(id, "textarea") {
            self.form.input_values.remove(&id);
            self.form.checkedness.remove(&id);
            self.form.indeterminate.remove(&id);
        } else if self.html_local_is(id, "select") {
            for option in self.select_options(id) {
                let selected = self.attribute(option, "selected").is_some();
                self.form.option_selectedness.insert(option, selected);
                self.form.option_dirty_selected.remove(&option);
            }
            self.apply_default_selectedness(id);
        }
    }

    /// An `input`'s checkedness: the stored value while the dirty checkedness
    /// flag is set, else whether the `checked` content attribute is present
    /// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-checked>).
    #[must_use]
    pub fn checkedness(&self, id: NodeId) -> bool {
        self.form
            .checkedness
            .get(&id)
            .copied()
            .unwrap_or_else(|| self.attribute(id, "checked").is_some())
    }

    /// Sets an input's checkedness, unchecking the rest of a radio button
    /// group when checking a radio
    /// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-state-(type=radio)>).
    pub fn set_input_checkedness(&mut self, id: NodeId, checked: bool) {
        if checked && self.input_type(id).as_deref() == Some("radio") {
            self.uncheck_radio_group(id);
        }
        self.form.checkedness.insert(id, checked);
    }

    /// Unchecks the other radios in `id`'s group. A radio button group is the
    /// radios in the same tree with the same form owner and name
    /// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
    fn uncheck_radio_group(&mut self, id: NodeId) {
        let Some(name) = self.attribute(id, "name") else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let owner = self.form_owner(id);
        let scope = self.tree_root_of(id);
        let others: Vec<NodeId> = self
            .descendants(scope)
            .filter(|&other| {
                other != id && self.is_radio_named(other, &name) && self.form_owner(other) == owner
            })
            .collect();
        for other in others {
            self.form.checkedness.insert(other, false);
        }
    }

    /// Re-applies a checked radio's group rule after its `name` or form owner
    /// changed or it moved trees
    /// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
    pub(crate) fn refresh_radio_group(&mut self, id: NodeId) {
        if self.checkedness(id) && self.input_type(id).as_deref() == Some("radio") {
            self.uncheck_radio_group(id);
        }
    }

    /// The form owner of `node` when it is a checked radio, else `None`; used
    /// to detect a form-owner change on insertion
    /// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
    pub(crate) fn checked_radio_form_owner(&self, node: NodeId) -> Option<NodeId> {
        if self.html_local_is(node, "input")
            && self.input_type(node).as_deref() == Some("radio")
            && self.checkedness(node)
        {
            self.form_owner(node)
        } else {
            None
        }
    }

    /// The form owner of a form-associated element: the form named by its
    /// `form` attribute, else its nearest ancestor form
    /// (<https://html.spec.whatwg.org/multipage/forms.html#form-owner>).
    #[must_use]
    pub fn form_owner(&self, id: NodeId) -> Option<NodeId> {
        if let Some(reference) = self.attribute(id, "form")
            && !reference.is_empty()
        {
            let root = self.tree_root_of(id);
            let found = self.descendants(root).find(|&other| {
                self.html_local_is(other, "form")
                    && self.attribute(other, "id").as_deref() == Some(reference.as_str())
            });
            if found.is_some() {
                return found;
            }
        }
        self.nearest_form_ancestor(id)
    }

    /// The checked radio in `id`'s radio button group, if any.
    #[must_use]
    pub fn radio_group_checked(&self, id: NodeId) -> Option<NodeId> {
        let name = self.attribute(id, "name")?;
        if name.is_empty() {
            return None;
        }
        let owner = self.form_owner(id);
        let scope = self.tree_root_of(id);
        self.descendants(scope).find(|&other| {
            other != id
                && self.is_radio_named(other, &name)
                && self.form_owner(other) == owner
                && self.checkedness(other)
        })
    }

    /// The nearest ancestor `form` of `node`, if any.
    fn nearest_form_ancestor(&self, node: NodeId) -> Option<NodeId> {
        let mut current = self.parent(node);
        while let Some(parent) = current {
            if self.html_local_is(parent, "form") {
                return Some(parent);
            }
            current = self.parent(parent);
        }
        None
    }

    /// The root of `node`'s tree, for grouping radios outside any form.
    #[must_use]
    pub fn tree_root_of(&self, node: NodeId) -> NodeId {
        let mut current = node;
        while let Some(parent) = self.parent(current) {
            current = parent;
        }
        current
    }

    /// Whether `node` is a radio with the given group name.
    fn is_radio_named(&self, node: NodeId, name: &str) -> bool {
        self.input_type(node).as_deref() == Some("radio")
            && self.attribute(node, "name").as_deref() == Some(name)
    }

    /// An `input`'s indeterminateness
    /// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-indeterminate>).
    #[must_use]
    pub fn indeterminate(&self, id: NodeId) -> bool {
        self.form.indeterminate.get(&id).copied().unwrap_or(false)
    }

    /// Sets `id`'s indeterminateness.
    pub fn set_indeterminate(&mut self, id: NodeId, indeterminate: bool) {
        self.form.indeterminate.insert(id, indeterminate);
    }

    /// The input/option cloning steps: propagate value, dirty value, and
    /// checkedness state from `from` to `to`
    /// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:cloning-steps>).
    pub fn clone_form_state(&mut self, from: NodeId, to: NodeId) {
        self.form.clone_dirty_value(from, to);
        if let Some(checked) = self.form.checkedness.get(&from).copied() {
            self.form.checkedness.insert(to, checked);
        }
        if let Some(indeterminate) = self.form.indeterminate.get(&from).copied() {
            self.form.indeterminate.insert(to, indeterminate);
        }
        if let Some(selected) = self.form.option_selectedness.get(&from).copied() {
            self.form.option_selectedness.insert(to, selected);
        }
        if self.form.option_dirty_selected.contains(&from) {
            self.form.option_dirty_selected.insert(to);
        }
    }
}
