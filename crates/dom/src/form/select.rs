//! The `select` and `option` model: option selectedness and its dirty flag, the
//! selectedness setting algorithm, and the `select` value/index IDL
//! (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element>).

use crate::{Dom, NodeId, NodeKind, html_namespace};

impl Dom {
    /// The nearest ancestor `select` of `node` (including `node`), if any.
    pub(crate) fn nearest_select_ancestor(&self, node: NodeId) -> Option<NodeId> {
        let mut current = Some(node);
        while let Some(candidate) = current {
            if self.html_local_is(candidate, "select") {
                return Some(candidate);
            }
            current = self.parent(candidate);
        }
        None
    }

    /// A `select`'s display size: the `size` attribute, else 4 with `multiple`
    /// and 1 otherwise
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element:display-size>).
    fn display_size(&self, select: NodeId) -> u32 {
        self.attribute(select, "size")
            .and_then(|raw| raw.trim().parse::<u32>().ok())
            .filter(|size| *size > 0)
            .unwrap_or_else(|| {
                if self.attribute(select, "multiple").is_some() {
                    4
                } else {
                    1
                }
            })
    }

    /// Whether an option is disabled by its own attribute or a disabled
    /// ancestor `optgroup`
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-disabled>).
    fn option_disabled(&self, option: NodeId) -> bool {
        if self.attribute(option, "disabled").is_some() {
            return true;
        }
        let mut current = self.parent(option);
        while let Some(ancestor) = current {
            if self.html_local_is(ancestor, "select")
                || self.html_local_is(ancestor, "hr")
                || self.html_local_is(ancestor, "datalist")
                || self.html_local_is(ancestor, "option")
            {
                return false;
            }
            if self.html_local_is(ancestor, "optgroup") {
                return self.attribute(ancestor, "disabled").is_some();
            }
            current = self.parent(ancestor);
        }
        false
    }

    /// The `select` selectedness setting algorithm: a single-select of display
    /// size 1 selects its first non-disabled option when nothing is selected,
    /// and keeps only the last selected option
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#selectedness-setting-algorithm>).
    pub(crate) fn apply_default_selectedness(&mut self, select: NodeId) {
        if self.attribute(select, "multiple").is_some() {
            return;
        }
        let options = self.select_options(select);
        let selected: Vec<NodeId> = options
            .iter()
            .copied()
            .filter(|&option| self.option_selected(option))
            .collect();
        if selected.len() >= 2 {
            for &option in &selected[..selected.len() - 1] {
                self.option_selectedness.insert(option, false);
            }
            return;
        }
        if selected.is_empty()
            && self.display_size(select) == 1
            && let Some(&first) = options.iter().find(|&&option| !self.option_disabled(option))
        {
            self.option_selectedness.insert(first, true);
        }
    }

    /// An `option`'s selectedness: the stored value while the dirty
    /// selectedness flag is set, else whether `selected` is present.
    #[must_use]
    pub fn option_selected(&self, id: NodeId) -> bool {
        self.option_selectedness
            .get(&id)
            .copied()
            .unwrap_or_else(|| self.attribute(id, "selected").is_some())
    }

    /// Sets `id`'s selectedness without the dirty flag, as the `Option`
    /// constructor does.
    pub fn set_option_selectedness(&mut self, id: NodeId, selected: bool) {
        self.option_selectedness.insert(id, selected);
    }

    /// Re-reads an `option`'s selectedness from its `selected` attribute when
    /// the dirty flag is clear.
    pub(crate) fn refresh_option_selectedness(&mut self, id: NodeId) {
        if self.html_local_is(id, "option") && !self.option_dirty_selected.contains(&id) {
            let selected = self.attribute(id, "selected").is_some();
            self.option_selectedness.insert(id, selected);
        }
    }

    /// The `select` ancestor of an `option`, if any.
    #[must_use]
    pub fn option_select_owner(&self, option: NodeId) -> Option<NodeId> {
        let mut current = self.parent(option);
        while let Some(parent) = current {
            if self.html_local_is(parent, "select") {
                return Some(parent);
            }
            current = self.parent(parent);
        }
        None
    }

    /// Sets an option's selectedness; selecting an option in a single-select
    /// clears the others.
    pub fn set_option_selected_in_select(&mut self, option: NodeId, selected: bool) {
        if selected
            && let Some(select) = self.option_select_owner(option)
            && self.attribute(select, "multiple").is_none()
        {
            for other in self.select_options(select) {
                self.option_selectedness.insert(other, false);
                self.option_dirty_selected.insert(other);
            }
        }
        self.option_selectedness.insert(option, selected);
        self.option_dirty_selected.insert(option);
    }

    /// The `option` elements under a `select`, in tree order.
    #[must_use]
    pub fn select_options(&self, select: NodeId) -> Vec<NodeId> {
        if !self.html_local_is(select, "select") {
            return Vec::new();
        }
        self.descendants(select)
            .filter(|&id| self.html_local_is(id, "option"))
            .collect()
    }

    /// An `option`'s value: its `value` attribute, else its text content
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-value>).
    #[must_use]
    pub fn option_value(&self, id: NodeId) -> String {
        self.no_namespace_attribute(id, "value")
            .unwrap_or_else(|| self.option_text(id))
    }

    /// An `option`'s text: its child text content with ASCII whitespace
    /// stripped and collapsed, skipping HTML and SVG `script` subtrees
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text>).
    #[must_use]
    pub fn option_text(&self, id: NodeId) -> String {
        let mut text = String::new();
        self.collect_option_text(id, &mut text);
        collapse_whitespace(&text)
    }

    /// Appends the option text under `id`, skipping HTML and SVG `script`.
    fn collect_option_text(&self, id: NodeId, text: &mut String) {
        let Some(children) = self.children(id) else {
            return;
        };
        for child in children {
            match self.kind(child) {
                Some(NodeKind::Text { data } | NodeKind::CDataSection { data }) => {
                    text.push_str(data);
                }
                Some(NodeKind::Element { name, .. }) => {
                    let is_script = name.local.as_ref().eq_ignore_ascii_case("script");
                    let skippable = name.ns == html_namespace()
                        || name.ns.as_ref() == "http://www.w3.org/2000/svg";
                    if !(is_script && skippable) {
                        self.collect_option_text(child, text);
                    }
                }
                _ => {}
            }
        }
    }

    /// A `select`'s value: the first selected option's value, else the empty
    /// string (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
    #[must_use]
    pub fn select_value(&self, id: NodeId) -> String {
        for option in self.select_options(id) {
            if self.option_selected(option) {
                return self.option_value(option);
            }
        }
        String::new()
    }

    /// A `select`'s selected index: the first selected option's index, else -1.
    #[must_use]
    pub fn select_selected_index(&self, id: NodeId) -> i32 {
        for (index, option) in self.select_options(id).iter().enumerate() {
            if self.option_selected(*option) {
                return i32::try_from(index).unwrap_or(i32::MAX);
            }
        }
        -1
    }

    /// Selects the option at `index`; a single-select clears the others first.
    pub fn set_select_selected_index(&mut self, id: NodeId, index: i32) {
        let options = self.select_options(id);
        if self.attribute(id, "multiple").is_none() {
            for &option in &options {
                self.option_selectedness.insert(option, false);
                self.option_dirty_selected.insert(option);
            }
        }
        if let Ok(index) = usize::try_from(index)
            && let Some(&option) = options.get(index)
        {
            self.option_selectedness.insert(option, true);
            self.option_dirty_selected.insert(option);
        }
    }

    /// Selects the first option whose value is `value`, clearing the rest
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
    pub fn set_select_value(&mut self, id: NodeId, value: &str) {
        let options = self.select_options(id);
        for &option in &options {
            self.option_selectedness.insert(option, false);
            self.option_dirty_selected.insert(option);
        }
        for option in options {
            if self.option_value(option) == value {
                self.option_selectedness.insert(option, true);
                self.option_dirty_selected.insert(option);
                break;
            }
        }
    }

}

/// Strips leading and trailing ASCII whitespace and collapses internal runs to
/// one space
/// (<https://infra.spec.whatwg.org/#strip-and-collapse-ascii-whitespace>).
fn collapse_whitespace(text: &str) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_ascii_whitespace() {
            pending_space = !result.is_empty();
        } else {
            if pending_space {
                result.push(' ');
                pending_space = false;
            }
            result.push(character);
        }
    }
    result
}

