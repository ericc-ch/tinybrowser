//! The `select` and `option` model: option selectedness and its dirty flag, the
//! selectedness setting algorithm, and the `select` value/index IDL
//! (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element>).

use crate::mutation;
use crate::{Document, DomError, LocalName, NodeId, NodeKind, QualName, html_namespace};

    /// Appends blank options as one tree mutation, then applies the select's
    /// selectedness setting algorithm
    /// (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#append-new-option-elements>).
    ///
    /// # Errors
    ///
    /// - [`DomError::StaleNode`] if `select` is stale.
    /// - [`DomError::WrongNodeType`] if `select` is not an HTML `select`.
    pub fn append_blank_options(document: &mut Document, select: NodeId, count: usize) -> Result<(), DomError> {
        if !document.contains(select) {
            return Err(DomError::StaleNode);
        }
        if !document.html_local_is(select, "select") {
            return Err(DomError::WrongNodeType);
        }
        if count == 0 {
            return Ok(());
        }
        let mut added = Vec::with_capacity(count);
        for _ in 0..count {
            added.push(document.create_element(
                QualName::new(None, html_namespace(), LocalName::from("option")),
                Vec::new(),
            ));
        }
        crate::mutation::append_fresh_children(document, select, added);
        apply_default_selectedness(document, select);
        Ok(())
    }

    /// A `select`'s display size: the `size` attribute parsed under the
    /// non-negative-integer rules, else 4 with `multiple` and 1 otherwise
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element:display-size>).
    fn display_size(document: &Document, select: NodeId) -> u32 {
        document.attribute(select, "size")
            .and_then(|raw| parse_non_negative_integer(&raw))
            .unwrap_or_else(|| {
                if document.attribute(select, "multiple").is_some() {
                    4
                } else {
                    1
                }
            })
    }

    /// [Rules for parsing non-negative integers](https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-non-negative-integers):
    /// ASCII whitespace, one optional sign, then leading ASCII digits.
    /// Trailing junk never fails the parse (`size="2abc"` is 2), and only a
    /// missing digit run is an error. Saturates on overflow: only the `== 1`
    /// comparison consumes this, which saturation preserves.
    fn parse_non_negative_integer(raw: &str) -> Option<u32> {
        let bytes = raw.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() && matches!(bytes[pos], b'\t' | b'\n' | b'\x0C' | b'\r' | b' ') {
            pos += 1;
        }
        if pos >= bytes.len() {
            return None;
        }
        let negative = match bytes[pos] {
            b'-' => {
                pos += 1;
                true
            }
            b'+' => {
                pos += 1;
                false
            }
            _ => false,
        };
        if pos >= bytes.len() || !bytes[pos].is_ascii_digit() {
            return None;
        }
        let mut magnitude: u64 = 0;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            magnitude = magnitude
                .saturating_mul(10)
                .saturating_add(u64::from(bytes[pos] - b'0'));
            pos += 1;
        }
        if negative {
            // `-0` still parses to zero; any other negative fails the
            // non-negative check.
            if magnitude == 0 { Some(0) } else { None }
        } else {
            Some(u32::try_from(magnitude).unwrap_or(u32::MAX))
        }
    }

    /// Whether an option is disabled by its own attribute or a disabled
    /// ancestor `optgroup`
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-disabled>).
    fn option_disabled(document: &Document, option: NodeId) -> bool {
        if document.attribute(option, "disabled").is_some() {
            return true;
        }
        let mut current = document.parent(option);
        while let Some(ancestor) = current {
            if document.html_local_is(ancestor, "select")
                || document.html_local_is(ancestor, "hr")
                || document.html_local_is(ancestor, "datalist")
                || document.html_local_is(ancestor, "option")
            {
                return false;
            }
            if document.html_local_is(ancestor, "optgroup") {
                return document.attribute(ancestor, "disabled").is_some();
            }
            current = document.parent(ancestor);
        }
        false
    }

    /// The `select` selectedness setting algorithm: a single-select of display
    /// size 1 selects its first non-disabled option when nothing is selected,
    /// and keeps only the last selected option
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#selectedness-setting-algorithm>).
    pub(crate) fn apply_default_selectedness(document: &mut Document, select: NodeId) {
        if document.attribute(select, "multiple").is_some() {
            return;
        }
        let options = select_options(document, select);
        let selected: Vec<NodeId> = options
            .iter()
            .copied()
            .filter(|&option| option_selected(document, option))
            .collect();
        if selected.len() >= 2 {
            for &option in &selected[..selected.len() - 1] {
                document.form.option_selectedness.insert(option, false);
            }
            return;
        }
        if selected.is_empty()
            && display_size(document, select) == 1
            && let Some(&first) = options
                .iter()
                .find(|&&option| !option_disabled(document, option))
        {
            document.form.option_selectedness.insert(first, true);
        }
    }

    /// An `option`'s selectedness: the stored value while the dirty
    /// selectedness flag is set, else whether `selected` is present.
    #[must_use]
    pub fn option_selected(document: &Document, id: NodeId) -> bool {
        document.form
            .option_selectedness
            .get(&id)
            .copied()
            .unwrap_or_else(|| document.attribute(id, "selected").is_some())
    }

    /// Sets `id`'s selectedness without the dirty flag, as the `Option`
    /// constructor does.
    pub fn set_option_selectedness(document: &mut Document, id: NodeId, selected: bool) {
        document.form.option_selectedness.insert(id, selected);
    }

    /// Re-reads an `option`'s selectedness from its `selected` attribute when
    /// the dirty flag is clear.
    pub(crate) fn refresh_option_selectedness(document: &mut Document, id: NodeId) {
        if document.html_local_is(id, "option") && !document.form.option_dirty_selected.contains(&id) {
            let selected = document.attribute(id, "selected").is_some();
            document.form.option_selectedness.insert(id, selected);
        }
    }

    /// The `select` whose list of options contains `node`'s subtree, if any.
    /// `node` is an `option` or `optgroup`; the walk stops at the boundaries
    /// the list-of-options algorithm excludes
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-option-list>).
    #[must_use]
    pub fn node_list_owner(document: &Document, node: NodeId) -> Option<NodeId> {
        let mut current = document.parent(node);
        let mut inside_optgroup = false;
        while let Some(ancestor) = current {
            if document.html_local_is(ancestor, "select") {
                return Some(ancestor);
            }
            if document.html_local_is(ancestor, "option")
                || document.html_local_is(ancestor, "hr")
                || document.html_local_is(ancestor, "datalist")
            {
                return None;
            }
            if document.html_local_is(ancestor, "optgroup") {
                if inside_optgroup {
                    return None;
                }
                inside_optgroup = true;
            }
            current = document.parent(ancestor);
        }
        None
    }

    /// The `select` whose list of options gains the inserted `option` or
    /// `optgroup` subtree, if any. An `optgroup` contributes through its
    /// descendant options, so a doubly nested group resolves to no select.
    #[must_use]
    pub(crate) fn inserted_list_owner(document: &Document, node: NodeId) -> Option<NodeId> {
        if document.html_local_is(node, "option") {
            return option_select_owner(document, node);
        }
        if document.html_local_is(node, "optgroup") {
            return document.tree().descendants(node)
                .filter(|&id| document.html_local_is(id, "option"))
                .find_map(|option| option_select_owner(document, option));
        }
        None
    }

    /// The `select` whose list of options contains this `option`, if any
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-option-list>).
    #[must_use]
    pub fn option_select_owner(document: &Document, option: NodeId) -> Option<NodeId> {
        if !document.html_local_is(option, "option") {
            return None;
        }
        node_list_owner(document, option)
    }

    /// The single-select rule for an option joining a list of options: when an
    /// option whose selectedness is true is added, every other option's
    /// selectedness becomes false. The prose near
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element>
    /// states this rule, but the selectedness setting algorithm's keep-last
    /// step would contradict it; WPT `inserted-or-removed.html` and Chromium's
    /// `DeselectItemsWithoutValidation` follow this rule, so this does too.
    pub(crate) fn option_added_to_select(document: &mut Document, option: NodeId) {
        let Some(select) = option_select_owner(document, option) else {
            return;
        };
        if document.attribute(select, "multiple").is_some() || !option_selected(document, option) {
            return;
        }
        for other in select_options(document, select) {
            if other != option {
                document.form.option_selectedness.insert(other, false);
            }
        }
    }

    /// Sets an option's selectedness; selecting an option in a single-select
    /// clears the others.
    pub fn set_option_selected_in_select(document: &mut Document, option: NodeId, selected: bool) {
        let owner = option_select_owner(document, option);
        if selected
            && let Some(select) = owner
            && document.attribute(select, "multiple").is_none()
        {
            for other in select_options(document, select) {
                document.form.option_selectedness.insert(other, false);
                document.form.option_dirty_selected.insert(other);
            }
        }
        document.form.option_selectedness.insert(option, selected);
        document.form.option_dirty_selected.insert(option);
        // An option whose selectedness becomes false asks its select to reset
        // (<https://html.spec.whatwg.org/multipage/form-elements.html#ask-for-a-reset>).
        if !selected && let Some(select) = owner {
            apply_default_selectedness(document, select);
        }
    }

    /// The `option` elements in a `select`'s list of options, in tree order
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-option-list>).
    #[must_use]
    pub fn select_options(document: &Document, select: NodeId) -> Vec<NodeId> {
        if !document.html_local_is(select, "select") {
            return Vec::new();
        }
        document.tree().descendants(select)
            .filter(|&id| option_select_owner(document, id) == Some(select))
            .collect()
    }

    /// An `option`'s value: its `value` attribute, else its text content
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-value>).
    #[must_use]
    pub fn option_value(document: &Document, id: NodeId) -> String {
        document.no_namespace_attribute(id, "value")
            .unwrap_or_else(|| option_text(document, id))
    }

    /// An `option`'s text: its child text content with ASCII whitespace
    /// stripped and collapsed, skipping HTML and SVG `script` subtrees
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text>).
    #[must_use]
    pub fn option_text(document: &Document, id: NodeId) -> String {
        let mut text = String::new();
        collect_option_text(document, id, &mut text);
        collapse_whitespace(&text)
    }

    /// Appends the option text under `id`, skipping HTML and SVG `script`.
    fn collect_option_text(document: &Document, id: NodeId, text: &mut String) {
        let Some(children) = document.children(id) else {
            return;
        };
        for child in children {
            match document.kind(child) {
                Some(NodeKind::Text { data } | NodeKind::CDataSection { data }) => {
                    text.push_str(data);
                }
                Some(NodeKind::Element { name, .. }) => {
                    let is_script = name.local.as_ref().eq_ignore_ascii_case("script");
                    let skippable = name.ns == html_namespace()
                        || name.ns.as_ref() == "http://www.w3.org/2000/svg";
                    if !(is_script && skippable) {
                        collect_option_text(document, child, text);
                    }
                }
                _ => {}
            }
        }
    }

    /// A `select`'s value: the first selected option's value, else the empty
    /// string (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
    #[must_use]
    pub fn select_value(document: &Document, id: NodeId) -> String {
        for option in select_options(document, id) {
            if option_selected(document, option) {
                return option_value(document, option);
            }
        }
        String::new()
    }

    /// A `select`'s selected index: the first selected option's index, else -1.
    #[must_use]
    pub fn select_selected_index(document: &Document, id: NodeId) -> i32 {
        for (index, option) in select_options(document, id).iter().enumerate() {
            if option_selected(document, *option) {
                return i32::try_from(index).unwrap_or(i32::MAX);
            }
        }
        -1
    }

    /// Selects the option at `index`; a single-select clears the others first.
    pub fn set_select_selected_index(document: &mut Document, id: NodeId, index: i32) {
        let options = select_options(document, id);
        if document.attribute(id, "multiple").is_none() {
            for &option in &options {
                document.form.option_selectedness.insert(option, false);
                document.form.option_dirty_selected.insert(option);
            }
        }
        if let Ok(index) = usize::try_from(index)
            && let Some(&option) = options.get(index)
        {
            document.form.option_selectedness.insert(option, true);
            document.form.option_dirty_selected.insert(option);
        }
    }

    /// Selects the first option whose value is `value`, clearing the rest
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
    pub fn set_select_value(document: &mut Document, id: NodeId, value: &str) {
        let options = select_options(document, id);
        for &option in &options {
            document.form.option_selectedness.insert(option, false);
            document.form.option_dirty_selected.insert(option);
        }
        for option in options {
            if option_value(document, option) == value {
                document.form.option_selectedness.insert(option, true);
                document.form.option_dirty_selected.insert(option);
                break;
            }
        }
    }

/// [Update descendant selectedcontent elements for an option](https://html.spec.whatwg.org/multipage/form-elements.html#update-descendant-selectedcontent-elements-for-an-option):
/// when the parser pops an option, the select's enabled selectedcontent
/// mirrors the first selected option in list order. The gate reads the
/// popped option's live selectedness; the insertion steps
/// (`place_node` → `option_added_to_select` + `apply_default_selectedness`)
/// already ran, so outside mid-parse script mutation the popped option is
/// the only selected one. Mid-parse scripts can dirty other options, so the
/// list is re-read to match the spec's first-selected rule instead of
/// assuming the popped option.
pub fn maybe_clone_option_into_selectedcontent(document: &mut Document, option: NodeId) {
    // The option's nearest ancestor select, with the list-of-options
    // boundaries (spec's "get the nearest ancestor select").
    let Some(select) = option_select_owner(document, option) else {
        return;
    };
    // Only a selected option triggers the update.
    if !option_selected(document, option) {
        return;
    }
    let Some(selectedcontent) = enabled_selectedcontent(document, select) else {
        return;
    };
    // "Update a selectedcontent": the first option in list order whose
    // selectedness is true, if any.
    let Some(first) = select_options(document, select)
        .into_iter()
        .find(|&id| option_selected(document, id))
    else {
        return;
    };
    clone_option_into_selectedcontent(document, first, selectedcontent);
}

/// Whether `id` is an HTML element with local name `local`, ASCII
/// case-insensitively. The parser lowercases tag names, so this matches the
/// exact check on parser-built trees; the insensitive comparison keeps
/// hand-built trees honest.
fn is_html_named(document: &Document, id: NodeId, local: &str) -> bool {
    match document.kind(id) {
        Some(NodeKind::Element { name, .. }) => {
            name.ns == html_namespace() && name.local.as_ref().eq_ignore_ascii_case(local)
        }
        _ => false,
    }
}

/// Whether the no-namespace attribute `local` is present on `id`, ASCII
/// case-insensitively.
fn html_bool_attr(document: &Document, id: NodeId, local: &str) -> bool {
    match document.kind(id) {
        Some(NodeKind::Element { attributes, .. }) => attributes.iter().any(|attribute| {
            attribute.name.ns.is_empty()
                && attribute.name.local.as_ref().eq_ignore_ascii_case(local)
        }),
        _ => false,
    }
}

/// [Enabled selectedcontent](https://html.spec.whatwg.org/multipage/form-elements.html#get-a-select-s-enabled-selectedcontent).
fn enabled_selectedcontent(document: &Document, select: NodeId) -> Option<NodeId> {
    if html_bool_attr(document, select, "multiple") {
        return None;
    }
    let mut pending: Vec<_> = document.children(select)?.rev().collect();
    while let Some(id) = pending.pop() {
        if is_html_named(document, id, "selectedcontent") {
            // https://html.spec.whatwg.org/multipage/form-elements.html#the-selectedcontent-element
            let mut ancestor = document.parent(id);
            while let Some(parent) = ancestor {
                if is_html_named(document, parent, "option")
                    || is_html_named(document, parent, "selectedcontent")
                {
                    return None;
                }
                ancestor = document.parent(parent);
            }
            return Some(id);
        }
        if let Some(kids) = document.children(id) {
            pending.extend(kids.rev());
        }
    }
    None
}

/// [Clone an option into a selectedcontent](https://html.spec.whatwg.org/multipage/form-elements.html#clone-an-option-into-a-selectedcontent).
fn clone_option_into_selectedcontent(
    document: &mut Document,
    option: NodeId,
    selectedcontent: NodeId,
) {
    let stale: Vec<NodeId> = document
        .children(selectedcontent)
        .map(Iterator::collect)
        .unwrap_or_default();
    for child in stale {
        mutation::destroy(document, child).expect("selectedcontent children are live descendants");
    }
    let kids: Vec<NodeId> = document
        .children(option)
        .map(Iterator::collect)
        .unwrap_or_default();
    for kid in kids {
        let cloned =
            mutation::clone_node(document, kid, true).expect("option children clone into new nodes");
        mutation::append(document, selectedcontent, cloned)
            .expect("selectedcontent accepts cloned option children");
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
