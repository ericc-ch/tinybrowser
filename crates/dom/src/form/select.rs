//! The `select` and `option` model: option selectedness and its dirty flag, the
//! selectedness setting algorithm, and the `select` value/index IDL
//! (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element>).

use crate::{Dom, DomError, LocalName, NodeId, NodeKind, QualName, html_namespace};

impl Dom {
    /// Appends blank options as one tree mutation, then applies the select's
    /// selectedness setting algorithm
    /// (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#append-new-option-elements>).
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `select` is stale.
    /// - [`DomError::WrongNodeType`] if `select` is not an HTML `select`.
    pub fn append_blank_options(&mut self, select: NodeId, count: usize) -> Result<(), DomError> {
        if !self.contains(select) {
            return Err(DomError::StaleNode);
        }
        if !self.html_local_is(select, "select") {
            return Err(DomError::WrongNodeType);
        }
        if count == 0 {
            return Ok(());
        }
        let mut added = Vec::with_capacity(count);
        for _ in 0..count {
            added.push(self.create_element(
                QualName::new(None, html_namespace(), LocalName::from("option")),
                Vec::new(),
            ));
        }
        self.append_fresh_children(select, added);
        self.apply_default_selectedness(select);
        Ok(())
    }

    /// A `select`'s display size: the `size` attribute, else 4 with `multiple`
    /// and 1 otherwise
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element:display-size>).
    fn display_size(&self, select: NodeId) -> u32 {
        self.attribute(select, "size")
            .and_then(|raw| {
                let raw = raw.trim_start_matches(['\t', '\n', '\u{c}', '\r', ' ']);
                let raw = raw.strip_prefix('+').unwrap_or(raw);
                let digits = raw.bytes().take_while(u8::is_ascii_digit).count();
                raw[..digits].parse::<u32>().ok()
            })
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
                self.form.option_selectedness.insert(option, false);
            }
            return;
        }
        if selected.is_empty()
            && self.display_size(select) == 1
            && let Some(&first) = options
                .iter()
                .find(|&&option| !self.option_disabled(option))
        {
            self.form.option_selectedness.insert(first, true);
        }
    }

    /// An `option`'s selectedness: the stored value while the dirty
    /// selectedness flag is set, else whether `selected` is present.
    #[must_use]
    pub fn option_selected(&self, id: NodeId) -> bool {
        self.form
            .option_selectedness
            .get(&id)
            .copied()
            .unwrap_or_else(|| self.attribute(id, "selected").is_some())
    }

    /// Sets `id`'s selectedness without the dirty flag, as the `Option`
    /// constructor does.
    pub fn set_option_selectedness(&mut self, id: NodeId, selected: bool) {
        self.form.option_selectedness.insert(id, selected);
    }

    /// Re-reads an `option`'s selectedness from its `selected` attribute when
    /// the dirty flag is clear.
    pub(crate) fn refresh_option_selectedness(&mut self, id: NodeId) {
        if self.html_local_is(id, "option") && !self.form.option_dirty_selected.contains(&id) {
            let selected = self.attribute(id, "selected").is_some();
            self.form.option_selectedness.insert(id, selected);
        }
    }

    /// The `select` whose list of options contains `node`'s subtree, if any.
    /// `node` is an `option` or `optgroup`; the walk stops at the boundaries
    /// the list-of-options algorithm excludes
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-option-list>).
    #[must_use]
    pub fn node_list_owner(&self, node: NodeId) -> Option<NodeId> {
        let mut current = self.parent(node);
        let mut inside_optgroup = false;
        while let Some(ancestor) = current {
            if self.html_local_is(ancestor, "select") {
                return Some(ancestor);
            }
            if self.html_local_is(ancestor, "option")
                || self.html_local_is(ancestor, "hr")
                || self.html_local_is(ancestor, "datalist")
            {
                return None;
            }
            if self.html_local_is(ancestor, "optgroup") {
                if inside_optgroup {
                    return None;
                }
                inside_optgroup = true;
            }
            current = self.parent(ancestor);
        }
        None
    }

    /// The `select` whose list of options gains the inserted `option` or
    /// `optgroup` subtree, if any. An `optgroup` contributes through its
    /// descendant options, so a doubly nested group resolves to no select.
    #[must_use]
    pub(crate) fn inserted_list_owner(&self, node: NodeId) -> Option<NodeId> {
        if self.html_local_is(node, "option") {
            return self.option_select_owner(node);
        }
        if self.html_local_is(node, "optgroup") {
            return self
                .descendants(node)
                .filter(|&id| self.html_local_is(id, "option"))
                .find_map(|option| self.option_select_owner(option));
        }
        None
    }

    /// The `select` whose list of options contains this `option`, if any
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-option-list>).
    #[must_use]
    pub fn option_select_owner(&self, option: NodeId) -> Option<NodeId> {
        if !self.html_local_is(option, "option") {
            return None;
        }
        self.node_list_owner(option)
    }

    /// The single-select rule for an option joining a list of options: when an
    /// option whose selectedness is true is added, every other option's
    /// selectedness becomes false. The prose near
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element>
    /// states this rule, but the selectedness setting algorithm's keep-last
    /// step would contradict it; WPT `inserted-or-removed.html` and Chromium's
    /// `DeselectItemsWithoutValidation` follow this rule, so this does too.
    pub(crate) fn option_added_to_select(&mut self, option: NodeId) {
        let Some(select) = self.option_select_owner(option) else {
            return;
        };
        if self.attribute(select, "multiple").is_some() || !self.option_selected(option) {
            return;
        }
        for other in self.select_options(select) {
            if other != option {
                self.form.option_selectedness.insert(other, false);
            }
        }
    }

    /// Sets an option's selectedness; selecting an option in a single-select
    /// clears the others.
    pub fn set_option_selected_in_select(&mut self, option: NodeId, selected: bool) {
        let owner = self.option_select_owner(option);
        if selected
            && let Some(select) = owner
            && self.attribute(select, "multiple").is_none()
        {
            for other in self.select_options(select) {
                self.form.option_selectedness.insert(other, false);
                self.form.option_dirty_selected.insert(other);
            }
        }
        self.form.option_selectedness.insert(option, selected);
        self.form.option_dirty_selected.insert(option);
        // An option whose selectedness becomes false asks its select to reset
        // (<https://html.spec.whatwg.org/multipage/form-elements.html#ask-for-a-reset>).
        if !selected && let Some(select) = owner {
            self.apply_default_selectedness(select);
        }
    }

    /// The `option` elements in a `select`'s list of options, in tree order
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-option-list>).
    #[must_use]
    pub fn select_options(&self, select: NodeId) -> Vec<NodeId> {
        if !self.html_local_is(select, "select") {
            return Vec::new();
        }
        self.descendants(select)
            .filter(|&id| self.option_select_owner(id) == Some(select))
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
                self.form.option_selectedness.insert(option, false);
                self.form.option_dirty_selected.insert(option);
            }
        }
        if let Ok(index) = usize::try_from(index)
            && let Some(&option) = options.get(index)
        {
            self.form.option_selectedness.insert(option, true);
            self.form.option_dirty_selected.insert(option);
        }
    }

    /// Selects the first option whose value is `value`, clearing the rest
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
    pub fn set_select_value(&mut self, id: NodeId, value: &str) {
        let options = self.select_options(id);
        for &option in &options {
            self.form.option_selectedness.insert(option, false);
            self.form.option_dirty_selected.insert(option);
        }
        for option in options {
            if self.option_value(option) == value {
                self.form.option_selectedness.insert(option, true);
                self.form.option_dirty_selected.insert(option);
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
