//! The form control model: form ownership, checkedness, radio button groups,
//! selectability, and the reset algorithm.
//!
//! The state lives in [`Dom`]'s side tables; this module owns the algorithms
//! over it. The tree mutation primitives in `arena` call back into these
//! methods at the points the spec defines as phenomena
//! (<https://html.spec.whatwg.org/multipage/forms.html#form-associated-element>).

mod input;
mod select;

use crate::{Dom, NodeId};

impl Dom {
    /// The reset algorithm for a form control: an `input`/`textarea` clears its
    /// dirty value and checkedness flags, and a `select` restores each option's
    /// selectedness from its `selected` attribute before running the
    /// selectedness setting algorithm
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-form-reset-control>).
    pub fn reset_control(&mut self, id: NodeId) {
        if self.html_local_is(id, "input") || self.html_local_is(id, "textarea") {
            self.input_values.remove(&id);
            self.checkedness.remove(&id);
            self.indeterminate.remove(&id);
        } else if self.html_local_is(id, "select") {
            for option in self.select_options(id) {
                let selected = self.attribute(option, "selected").is_some();
                self.option_selectedness.insert(option, selected);
                self.option_dirty_selected.remove(&option);
            }
            self.apply_default_selectedness(id);
        }
    }

    /// An `input`'s checkedness: the stored value while the dirty checkedness
    /// flag is set, else whether the `checked` content attribute is present
    /// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-checked>).
    #[must_use]
    pub fn checkedness(&self, id: NodeId) -> bool {
        self.checkedness
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
        self.checkedness.insert(id, checked);
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
                other != id
                    && self.is_radio_named(other, &name)
                    && self.form_owner(other) == owner
            })
            .collect();
        for other in others {
            self.checkedness.insert(other, false);
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
        self.indeterminate.get(&id).copied().unwrap_or(false)
    }

    /// Sets `id`'s indeterminateness.
    pub fn set_indeterminate(&mut self, id: NodeId, indeterminate: bool) {
        self.indeterminate.insert(id, indeterminate);
    }

    /// The input/option cloning steps: propagate value, dirty value, and
    /// checkedness state from `from` to `to`
    /// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:cloning-steps>).
    pub fn clone_form_state(&mut self, from: NodeId, to: NodeId) {
        if let Some(value) = self.input_values.get(&from).cloned() {
            self.input_values.insert(to, value);
        }
        if let Some(checked) = self.checkedness.get(&from).copied() {
            self.checkedness.insert(to, checked);
        }
        if let Some(indeterminate) = self.indeterminate.get(&from).copied() {
            self.indeterminate.insert(to, indeterminate);
        }
        if let Some(selected) = self.option_selectedness.get(&from).copied() {
            self.option_selectedness.insert(to, selected);
        }
        if self.option_dirty_selected.contains(&from) {
            self.option_dirty_selected.insert(to);
        }
    }

}
