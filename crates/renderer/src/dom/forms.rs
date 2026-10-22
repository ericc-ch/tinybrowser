//! Form-control state stored alongside the tree.
//!
//! Inputs keep the sanitized current value in Blitz's dirty-value slot with a
//! pristine marker, and options keep selectedness plus dirtiness in
//! `BlitzDocument`. The IDL getters, collections, submission, and insertion
//! fixups all read those slots through the helpers here
//! (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html>).

use blitz_dom::{BaseDocument, NodeData};

use crate::Parsed;
use crate::dom::tree::{BlitzId, NodeId, attr, form_owner_of, html_namespace, is_html_element};

/// A control's contribution to the entry list, resolved inside one world
/// borrow; file controls read their JS `files` after the borrow ends.
pub(crate) enum PendingEntry {
    Text(String, String),
    Files(String, NodeId),
}

/// Form controls in a subtree, in tree order, for insertion fixups after a
/// whole-subtree replacement such as `innerHTML`
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element>).
pub(crate) fn descendant_controls(
    base: &BaseDocument,
    document: u32,
    root: BlitzId,
) -> Vec<NodeId> {
    let mut controls = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let Some(target) = base.get_node(node) else {
            continue;
        };
        stack.extend(target.children.iter().rev().copied());
        let Some(element) = target.data.downcast_element() else {
            continue;
        };
        if element.name.ns == html_namespace()
            && matches!(element.name.local.as_str(), "option" | "input")
        {
            controls.push(NodeId { document, node });
        }
    }
    controls
}

/// Materializes each input's sanitized value and each select's selectedness
/// once after parsing, so layout and `:checked` see the same state as the IDL
/// getters before any script runs
/// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element>,
/// <https://html.spec.whatwg.org/multipage/form-elements.html#selectedness-setting-algorithm>).
pub(crate) fn initialize_controls(parsed: &mut Parsed) {
    let Some(root) = parsed.document.base.try_root_element() else {
        return;
    };
    let mut inputs = Vec::new();
    let mut selects = Vec::new();
    let mut stack = vec![root.id];
    while let Some(node) = stack.pop() {
        let Some(target) = parsed.document.base.get_node(node) else {
            continue;
        };
        stack.extend(target.children.iter().rev().copied());
        let Some(element) = target.data.downcast_element() else {
            continue;
        };
        if element.name.ns != html_namespace() {
            continue;
        }
        match element.name.local.as_str() {
            "input" => inputs.push(node),
            "select" => selects.push(node),
            _ => {}
        }
    }
    for node in inputs {
        crate::dom::input::sync_sanitized_value(parsed, node);
    }
    for select in selects {
        parsed.document.invalidate_selectedness(select);
        run_selectedness_setting(parsed, select);
    }
}

/// Collects a form's entry list: every submittable control whose form owner is
/// the form, in tree order, that carries a name and is not disabled or inside a
/// `datalist`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set>).
pub(crate) fn collect_pending_entries(
    parsed: &mut Parsed,
    form: NodeId,
    submitter: Option<NodeId>,
) -> Vec<PendingEntry> {
    let base = &parsed.document.base;
    let document = parsed.id;
    // The tree `form` participates in; form owners never cross trees.
    let mut root = form.node;
    while let Some(parent) = base.get_node(root).and_then(|node| node.parent) {
        root = parent;
    }
    // A pre-order walk visits nodes in tree order.
    let mut pending = Vec::new();
    let mut controls = Vec::new();
    let mut stack = vec![root];
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        stack.extend(node.children.iter().rev().copied());
        let Some(element) = node.data.downcast_element() else {
            continue;
        };
        if element.name.ns != html_namespace() {
            continue;
        }
        let local = element.name.local.as_str();
        if !matches!(local, "input" | "textarea" | "select" | "button") {
            continue;
        }
        if form_owner_of(base, document, current) != Some(form) {
            continue;
        }
        let Some(control_name) = attr(base, current, "name") else {
            continue;
        };
        if control_name.is_empty()
            || is_disabled(base, current)
            || has_datalist_ancestor(base, current)
        {
            continue;
        }
        // A hidden input named `_charset_` carries the encoding name
        // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fe-name-charset>).
        if local == "input"
            && crate::dom::input::input_type(base, current)
                == Some(crate::dom::input::InputType::Hidden)
            && control_name.eq_ignore_ascii_case("_charset_")
        {
            pending.push(PendingEntry::Text(
                control_name.to_owned(),
                "UTF-8".to_owned(),
            ));
            continue;
        }
        controls.push((current, local.to_owned(), control_name.to_owned()));
    }
    for (current, local, control_name) in controls {
        let is_submitter = submitter
            == Some(NodeId {
                document,
                node: current,
            });
        pending.extend(pending_entry(
            parsed,
            document,
            current,
            &local,
            &control_name,
            is_submitter,
        ));
    }
    pending
}

/// Whether `node` is disabled: its own `disabled` attribute, or for an
/// `option` a `disabled` ancestor `optgroup` or owning `select`. Inheritance
/// through an ancestor `fieldset` (outside its first `legend`) is a known gap
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-fe-disabled>).
pub(crate) fn is_disabled(base: &BaseDocument, node: BlitzId) -> bool {
    if attr(base, node, "disabled").is_some() {
        return true;
    }
    if !is_html_element(base, node, "option") {
        return false;
    }
    let mut cursor = base.get_node(node).and_then(|target| target.parent);
    while let Some(current) = cursor {
        let Some(element) = base
            .get_node(current)
            .and_then(|target| target.data.downcast_element())
        else {
            cursor = base.get_node(current).and_then(|target| target.parent);
            continue;
        };
        if element.name.ns != html_namespace() {
            cursor = base.get_node(current).and_then(|target| target.parent);
            continue;
        }
        match element.name.local.as_str() {
            "optgroup" | "select" if attr(base, current, "disabled").is_some() => return true,
            "select" => return false,
            _ => {}
        }
        cursor = base.get_node(current).and_then(|target| target.parent);
    }
    false
}

/// Whether `node` has a `datalist` ancestor, which bars it from the entry list
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set>).
pub(crate) fn has_datalist_ancestor(base: &BaseDocument, node: BlitzId) -> bool {
    let mut cursor = base.get_node(node).and_then(|target| target.parent);
    while let Some(current) = cursor {
        if is_html_element(base, current, "datalist") {
            return true;
        }
        cursor = base.get_node(current).and_then(|target| target.parent);
    }
    false
}

/// One named, enabled control's contribution: nothing, one entry, or (for a
/// multiple `select`) several
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set>).
pub(crate) fn pending_entry(
    parsed: &mut Parsed,
    document: u32,
    node: BlitzId,
    local: &str,
    control_name: &str,
    is_submitter: bool,
) -> Vec<PendingEntry> {
    let id = NodeId { document, node };
    let base = &parsed.document.base;
    // A dirty value wins. Otherwise the submission value is the `value`
    // content attribute (the default value), or empty when absent.
    let value_attr = || attr(base, node, "value").unwrap_or_default().to_owned();
    match local {
        "textarea" => {
            let mut value = textarea_api_value(base, node);
            if wrap_is_hard(attr(base, node, "wrap")) {
                let cols = parse_positive(attr(base, node, "cols")).unwrap_or(20);
                value = hard_wrap(&value, cols);
            }
            vec![PendingEntry::Text(control_name.to_owned(), value)]
        }
        "button" => {
            // A button contributes only when it is the submitter
            // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set>).
            if !is_submitter {
                return Vec::new();
            }
            vec![PendingEntry::Text(control_name.to_owned(), value_attr())]
        }
        "input" => {
            use crate::dom::input::{InputType, current_value, input_type};
            let kind = input_type(base, node).unwrap_or(InputType::Text);
            match kind {
                InputType::Reset | InputType::Button => Vec::new(),
                InputType::Submit | InputType::Image if !is_submitter => Vec::new(),
                InputType::File => vec![PendingEntry::Files(control_name.to_owned(), id)],
                InputType::Checkbox | InputType::Radio => {
                    // Checkable controls contribute only when checked; their
                    // current value is the attribute or "on"
                    // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value-default-on>).
                    let checked = base
                        .get_node(node)
                        .and_then(|target| target.data.downcast_element())
                        .is_some_and(blitz_dom::ElementData::checkedness);
                    if checked {
                        vec![PendingEntry::Text(
                            control_name.to_owned(),
                            current_value(parsed, node),
                        )]
                    } else {
                        Vec::new()
                    }
                }
                _ => vec![PendingEntry::Text(
                    control_name.to_owned(),
                    current_value(parsed, node),
                )],
            }
        }
        "select" => {
            let multiple = attr(base, node, "multiple").is_some();
            if multiple {
                let selected = effective_selected_options(parsed, node);
                let base = &parsed.document.base;
                selected
                    .into_iter()
                    .filter(|option| !is_disabled(base, option.node))
                    .map(|option| {
                        PendingEntry::Text(control_name.to_owned(), option_value(base, option.node))
                    })
                    .collect()
            } else {
                let value = select_value(parsed, node);
                vec![PendingEntry::Text(control_name.to_owned(), value)]
            }
        }
        _ => Vec::new(),
    }
}

/// The `option` elements in a `select`'s list of options, in tree order:
/// descendant options whose nearest ancestor `select` is this one. The walk
/// stops descending at a nested `select`, so its options belong to the inner
/// select and never to this one, and no per-option owner lookup is needed
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-option-list>).
pub(crate) fn select_options(base: &BaseDocument, document: u32, select: BlitzId) -> Vec<NodeId> {
    let mut options = Vec::new();
    let mut stack: Vec<BlitzId> = base
        .get_node(select)
        .map(|node| node.children.iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        if node.data.downcast_element().is_some_and(|element| {
            element.name.ns == html_namespace() && element.name.local.as_str() == "select"
        }) {
            continue;
        }
        if node.data.downcast_element().is_some_and(|element| {
            element.name.ns == html_namespace() && element.name.local.as_str() == "option"
        }) {
            options.push(NodeId {
                document,
                node: current,
            });
        }
        stack.extend(node.children.iter().rev().copied());
    }
    options
}

/// The `select` whose list of options contains `node`: its nearest ancestor
/// `select` element, if any.
pub(crate) fn option_select_owner(base: &BaseDocument, node: BlitzId) -> Option<BlitzId> {
    let mut cursor = base.get_node(node).and_then(|target| target.parent);
    while let Some(current) = cursor {
        if is_html_element(base, current, "select") {
            return Some(current);
        }
        cursor = base.get_node(current).and_then(|target| target.parent);
    }
    None
}

/// Whether an option is disabled: the option itself or its parent optgroup.
/// A disabled select does not disable its options
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-disabled>).
fn option_disabled(base: &BaseDocument, option: BlitzId) -> bool {
    if attr(base, option, "disabled").is_some() {
        return true;
    }
    let mut cursor = base.get_node(option).and_then(|node| node.parent);
    while let Some(current) = cursor {
        if is_html_element(base, current, "select") {
            return false;
        }
        if is_html_element(base, current, "optgroup") && attr(base, current, "disabled").is_some() {
            return true;
        }
        cursor = base.get_node(current).and_then(|node| node.parent);
    }
    false
}

/// The select's display size. The `size` attribute wins even when it parses
/// to zero; otherwise a multiple select shows four rows and a single select
/// one (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-size>).
fn select_display_size(base: &BaseDocument, select: BlitzId) -> u32 {
    if let Some(size) =
        attr(base, select, "size").and_then(crate::dom::parse::parse_non_negative_integer)
    {
        return size;
    }
    if attr(base, select, "multiple").is_some() {
        4
    } else {
        1
    }
}

/// Reads an option's selectedness slot, initializing it from the `selected`
/// attribute the first time the option is observed. Dirtiness stays false, so
/// a later attribute change applies.
fn option_selectedness(parsed: &mut Parsed, option: BlitzId) -> bool {
    if let Some(selected) = parsed.document.option_selectedness(option) {
        return selected;
    }
    let selected = attr(&parsed.document.base, option, "selected").is_some();
    parsed.document.set_option_selectedness(option, selected);
    selected
}

fn set_option_selectedness(parsed: &mut Parsed, option: BlitzId, selected: bool) {
    parsed.document.set_option_selectedness(option, selected);
    parsed
        .document
        .base
        .mutate()
        .set_selectedness(option, selected);
}

/// Clears selectedness on every option except `keep`. Setting an option's
/// selectedness to true in a single select deselects the previous option
/// before the selectedness setting algorithm runs
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-selectedness>).
fn clear_other_options(parsed: &mut Parsed, select: BlitzId, keep: BlitzId) {
    for option in select_options(&parsed.document.base, parsed.id, select) {
        if option.node != keep
            && parsed
                .document
                .option_selectedness(option.node)
                .unwrap_or(false)
        {
            set_option_selectedness(parsed, option.node, false);
        }
    }
}

/// Prepares an option that was just inserted: selectedness from its attribute
/// is materialized, and a selected option in a single select replaces the
/// previous selection
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element>).
pub(crate) fn option_inserted(parsed: &mut Parsed, option: BlitzId) {
    let Some(select) = option_select_owner(&parsed.document.base, option) else {
        return;
    };
    // An option's selected attribute is the default selectedness while its
    // dirtiness is false. Read the attribute here because an earlier option in
    // the same insertion batch may have cleared the stored slot already
    // (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-dirtiness>).
    let selected = if parsed.document.option_dirty(option) {
        option_selectedness(parsed, option)
    } else {
        // A parsed selected attribute or a slot kept from an earlier tree
        // position both select the inserted option. An earlier option in the
        // same batch may have cleared the slot, so the attribute still wins.
        attr(&parsed.document.base, option, "selected").is_some()
            || parsed.document.option_selectedness(option).unwrap_or(false)
    };
    parsed.document.set_option_selectedness(option, selected);
    if selected && attr(&parsed.document.base, select, "multiple").is_none() {
        clear_other_options(parsed, select, option);
    }
    parsed.document.invalidate_selectedness(select);
    run_selectedness_setting(parsed, select);
}

/// Sets an option's selectedness from the `Option()` constructor. The
/// constructor overrides the initial selectedness even when the third argument
/// set the `selected` attribute, and never sets dirtiness
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-option>).
pub(crate) fn initialize_option_selectedness(parsed: &mut Parsed, option: BlitzId, selected: bool) {
    set_option_selectedness(parsed, option, selected);
    parsed.document.clear_option_dirty(option);
}

/// The selectedness setting algorithm
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#selectedness-setting-algorithm>).
/// Materializes every option's selectedness and returns whether anything
/// changed. A select is normalized once; DOM mutations invalidate the cache.
pub(crate) fn run_selectedness_setting(parsed: &mut Parsed, select: BlitzId) -> bool {
    if parsed.document.selectedness_normalized(select) {
        return false;
    }
    let options = select_options(&parsed.document.base, parsed.id, select);
    if attr(&parsed.document.base, select, "multiple").is_some() {
        for option in &options {
            option_selectedness(parsed, option.node);
        }
        parsed.document.mark_selectedness_normalized(select);
        return false;
    }
    let current: Vec<bool> = options
        .iter()
        .map(|option| option_selectedness(parsed, option.node))
        .collect();
    let last_selected = current.iter().rposition(|selected| *selected);
    let first_enabled = options
        .iter()
        .position(|option| !option_disabled(&parsed.document.base, option.node));
    let mut desired = vec![false; options.len()];
    if let Some(index) = last_selected {
        desired[index] = true;
    } else if select_display_size(&parsed.document.base, select) == 1
        && let Some(index) = first_enabled
    {
        desired[index] = true;
    }
    let changed = desired
        .iter()
        .zip(&current)
        .any(|(desired, current)| desired != current);
    for (option, selected) in options.iter().zip(desired) {
        set_option_selectedness(parsed, option.node, selected);
    }
    parsed.document.mark_selectedness_normalized(select);
    changed
}

/// The options a select has selected, in tree order, after materializing the
/// selectedness setting algorithm.
pub(crate) fn effective_selected_options(parsed: &mut Parsed, select: BlitzId) -> Vec<NodeId> {
    run_selectedness_setting(parsed, select);
    let base = &parsed.document.base;
    select_options(base, parsed.id, select)
        .into_iter()
        .filter(|option| {
            parsed
                .document
                .option_selectedness(option.node)
                .unwrap_or_else(|| option_selectedness(parsed, option.node))
        })
        .collect()
}

/// An option's selectedness, including the reset behavior for a single select
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#ask-for-a-reset>).
pub(crate) fn option_effective_selected(parsed: &mut Parsed, option: BlitzId) -> bool {
    let Some(select) = option_select_owner(&parsed.document.base, option) else {
        return option_selectedness(parsed, option);
    };
    run_selectedness_setting(parsed, select);
    parsed
        .document
        .option_selectedness(option)
        .unwrap_or_else(|| option_selectedness(parsed, option))
}

/// An `option`'s value: its `value` attribute, else its text
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-value>).
pub(crate) fn option_value(base: &BaseDocument, option: BlitzId) -> String {
    attr(base, option, "value").map_or_else(|| option_text(base, option), str::to_owned)
}

/// An `option`'s text: its descendant text with ASCII whitespace stripped and
/// collapsed, skipping HTML and SVG `script` subtrees
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text>).
pub(crate) fn option_text(base: &BaseDocument, option: BlitzId) -> String {
    let mut text = String::new();
    let mut stack: Vec<BlitzId> = base
        .get_node(option)
        .map(|node| node.children.iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        match &node.data {
            NodeData::Text(data) => text.push_str(&data.content),
            NodeData::CDataSection { contents } => text.push_str(contents),
            NodeData::Element(element) => {
                let is_script = element.name.local.as_str().eq_ignore_ascii_case("script")
                    && (element.name.ns == html_namespace()
                        || element.name.ns.as_str() == "http://www.w3.org/2000/svg");
                if !is_script {
                    stack.extend(node.children.iter().rev().copied());
                }
            }
            _ => {}
        }
    }
    collapse_whitespace(&text)
}

/// A `select`'s value: the first selected option's value, else the empty
/// string (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
pub(crate) fn select_value(parsed: &mut Parsed, select: BlitzId) -> String {
    effective_selected_options(parsed, select)
        .first()
        .map_or_else(String::new, |option| {
            option_value(&parsed.document.base, option.node)
        })
}

/// The select's selected index: the first selected option, or `-1`
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex>).
pub(crate) fn selected_index_of(parsed: &mut Parsed, select: BlitzId) -> i32 {
    let Some(selected) = effective_selected_options(parsed, select)
        .into_iter()
        .next()
    else {
        return -1;
    };
    select_options(&parsed.document.base, parsed.id, select)
        .iter()
        .position(|option| *option == selected)
        .map_or(-1, |index| i32::try_from(index).unwrap_or(-1))
}

/// The select reset algorithm: restore selectedness from the `selected`
/// attributes, clear dirtiness, then run the selectedness setting algorithm
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element>).
pub(crate) fn reset_select_selectedness(parsed: &mut Parsed, select: BlitzId) {
    for option in select_options(&parsed.document.base, parsed.id, select) {
        let selected = attr(&parsed.document.base, option.node, "selected").is_some();
        parsed.document.clear_option_dirty(option.node);
        set_option_selectedness(parsed, option.node, selected);
    }
    parsed.document.invalidate_selectedness(select);
    run_selectedness_setting(parsed, select);
}

/// Strips and collapses ASCII whitespace runs to single spaces, dropping
/// leading and trailing runs.
pub(crate) fn collapse_whitespace(text: &str) -> String {
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

/// The submission encoding for a resolved label: the same ASCII-trimmed,
/// `replacement`-rejecting resolution as every other label entry, falling
/// back to UTF-8 because the caller passes an already-resolved name.
pub(crate) fn submission_encoding(label: &str) -> &'static encoding_rs::Encoding {
    encoding_rs::Encoding::for_label(trim_label(label).as_bytes())
        .filter(|encoding| *encoding != encoding_rs::REPLACEMENT)
        .unwrap_or(encoding_rs::UTF_8)
}

/// Encodes `text` in the form's submission encoding, returning a Latin-1
/// string of the encoded bytes. A character the encoding cannot represent
/// becomes a numeric character reference, per the Encoding Standard's
/// `encode` (<https://encoding.spec.whatwg.org/#encode>).
pub(crate) fn encode_form(text: &str, label: &str) -> String {
    let (bytes, _, _) = submission_encoding(label).encode(text);
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// Percent-encodes `text` for form submission: Encoding Standard `encode`
/// in `label`, then the urlencoded byte serializer over those bytes
/// (<https://url.spec.whatwg.org/#concept-urlencoded-byte-serializer>).
pub(crate) fn urlencode_form(text: &str, label: &str) -> String {
    let (bytes, _, _) = submission_encoding(label).encode(text);
    url::form_urlencoded::byte_serialize(bytes.as_ref()).collect()
}

/// Whether a `textarea`'s `wrap` attribute is in the Hard state
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#attr-textarea-wrap>):
/// an enumerated attribute, ASCII case-insensitive.
pub(crate) fn wrap_is_hard(wrap: Option<&str>) -> bool {
    wrap.is_some_and(|wrap| wrap.trim().eq_ignore_ascii_case("hard"))
}

/// The positive integer in `value`, or `None` for an absent, invalid, or
/// zero value. HTML's parsing rules for non-negative integers accept zero;
/// callers of this helper (a `textarea`'s `cols`) require a value greater
/// than zero
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-non-negative-integers>).
pub(crate) fn parse_positive(value: Option<&str>) -> Option<usize> {
    let parsed = crate::dom::parse::parse_non_negative_integer(value?)?;
    if parsed == 0 {
        return None;
    }
    Some(usize::try_from(parsed).unwrap_or(usize::MAX))
}

/// The textarea hard-wrapping transformation: break each line at `cols`
/// characters and join with LF. The submitted value normalizes to LF here; the
/// urlencoded serializer turns those into CRLF pairs
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#textarea-wrapping-transformation>).
pub(crate) fn hard_wrap(value: &str, cols: usize) -> String {
    let cols = cols.max(1);
    let mut out = String::with_capacity(value.len());
    for (index, line) in value.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let characters: Vec<char> = line.chars().collect();
        let mut start = 0;
        while start < characters.len() {
            if start > 0 {
                out.push('\n');
            }
            let end = (start + cols).min(characters.len());
            out.extend(&characters[start..end]);
            start = end;
        }
    }
    out
}

/// Strips ASCII whitespace, the only whitespace the Encoding Standard's
/// label matching removes
/// (<https://encoding.spec.whatwg.org/#concept-encoding-get>).
pub(crate) fn trim_label(label: &str) -> &str {
    label.trim_matches(['\t', '\n', '\x0C', '\r', ' '])
}

/// Radios sharing a non-empty `name` in the same document, in tree order,
/// excluding `except`. An empty or missing name forms no group, so a nameless
/// radio never unchecks another
/// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
/// Blitz keeps no form-owner model, so the group spans the document instead
/// of the form (a known cutover gap).
pub(crate) fn radio_group_members(
    base: &blitz_dom::BaseDocument,
    document: u32,
    name: &str,
    except: BlitzId,
) -> Vec<NodeId> {
    if name.is_empty() {
        return Vec::new();
    }
    let mut group = Vec::new();
    let mut stack = vec![base.root_node().id];
    // Reverse-push keeps the pop order in tree order.
    while let Some(id) = stack.pop() {
        let Some(node) = base.get_node(id) else {
            continue;
        };
        if id != except
            && node.data.downcast_element().is_some_and(|element| {
                element.name.ns == html_namespace()
                    && element.name.local.as_str() == "input"
                    && attr(base, id, "type")
                        .unwrap_or("text")
                        .trim()
                        .eq_ignore_ascii_case("radio")
                    && attr(base, id, "name").unwrap_or_default() == name
            })
        {
            group.push(NodeId { document, node: id });
        }
        stack.extend(node.children.iter().rev().copied());
    }
    group
}

/// A control's value length in UTF-16 code units, the basis selection
/// offsets clamp against: the `value` attribute for inputs, descendant text
/// for textareas.
pub(crate) fn control_value_len(parsed: &Parsed, node: BlitzId) -> usize {
    let base = &parsed.document.base;
    let Some(element) = base
        .get_node(node)
        .and_then(|target| target.data.downcast_element())
    else {
        return 0;
    };
    if element.name.ns != html_namespace() {
        return 0;
    }
    match element.name.local.as_str() {
        "textarea" => textarea_api_value(base, node).encode_utf16().count(),
        "input" => crate::dom::input::current_value(parsed, node)
            .encode_utf16()
            .count(),
        _ => 0,
    }
}

/// The textarea API value: the raw value (dirty slot or child text content)
/// with newlines normalized to LF
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-textarea-api-value>).
pub(crate) fn textarea_api_value(base: &blitz_dom::BaseDocument, node: BlitzId) -> String {
    let raw = base
        .get_node(node)
        .and_then(|target| target.data.downcast_element())
        .filter(|element| element.form_state.dirty_value)
        .map_or_else(
            || crate::dom::text::child_text_content(base, node),
            |element| element.form_state.value.clone(),
        );
    crate::dom::text::normalize_newlines(&raw)
}

/// Whether text selection applies to a control: textareas always, text-like
/// inputs only.
pub(crate) fn selection_applies(base: &blitz_dom::BaseDocument, node: BlitzId) -> bool {
    let Some(element) = base
        .get_node(node)
        .and_then(|target| target.data.downcast_element())
    else {
        return false;
    };
    if element.name.ns != html_namespace() {
        return false;
    }
    match element.name.local.as_str() {
        "textarea" => true,
        "input" => crate::dom::input::input_type(base, node)
            .is_some_and(crate::dom::input::InputType::selection_applies),
        _ => false,
    }
}

/// Radios to uncheck when checked `input` joins a named group: the other
/// group members.
pub(crate) fn radios_to_uncheck(base: &BaseDocument, document: u32, input: BlitzId) -> Vec<NodeId> {
    let Some(element) = base
        .get_node(input)
        .and_then(|node| node.data.downcast_element())
    else {
        return Vec::new();
    };
    if element.name.ns != html_namespace() || element.name.local.as_str() != "input" {
        return Vec::new();
    }
    if attr(base, input, "type")
        .unwrap_or("text")
        .trim()
        .eq_ignore_ascii_case("radio")
        && attr(base, input, "checked").is_some()
    {
        let name = attr(base, input, "name").unwrap_or_default().to_owned();
        radio_group_members(base, document, &name, input)
    } else {
        Vec::new()
    }
}

/// The `option.selected` setter: set selectedness and dirtiness, then ask the
/// owning select for a reset
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected>).
pub(crate) fn set_option_selected(parsed: &mut Parsed, option: BlitzId, selected: bool) {
    if let Some(select) = option_select_owner(&parsed.document.base, option)
        && selected
        && attr(&parsed.document.base, select, "multiple").is_none()
    {
        clear_other_options(parsed, select, option);
    }
    set_option_selectedness(parsed, option, selected);
    parsed.document.mark_option_dirty(option);
    if let Some(select) = option_select_owner(&parsed.document.base, option) {
        parsed.document.invalidate_selectedness(select);
        run_selectedness_setting(parsed, select);
    }
}

/// The `select.selectedIndex` setter: clear every selectedness, then select
/// the requested index and mark it dirty. No default is applied afterwards
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex>).
pub(crate) fn set_select_selected_index_value(parsed: &mut Parsed, select: BlitzId, value: i32) {
    let options = select_options(&parsed.document.base, parsed.id, select);
    let wanted = usize::try_from(value).ok();
    for (index, option) in options.into_iter().enumerate() {
        let selected = Some(index) == wanted;
        set_option_selectedness(parsed, option.node, selected);
        if selected {
            parsed.document.mark_option_dirty(option.node);
        }
    }
    parsed.document.mark_selectedness_normalized(select);
}

/// The `select.value` setter: clear every selectedness, select the first
/// matching option, and mark it dirty. No default is applied when nothing
/// matches (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
pub(crate) fn set_select_value_selectedness(parsed: &mut Parsed, select: BlitzId, value: &str) {
    let options = select_options(&parsed.document.base, parsed.id, select);
    let mut first_matching = None;
    for option in &options {
        if first_matching.is_none() && option_value(&parsed.document.base, option.node) == value {
            first_matching = Some(option.node);
        }
    }
    for option in options {
        let selected = first_matching == Some(option.node);
        set_option_selectedness(parsed, option.node, selected);
        if selected {
            parsed.document.mark_option_dirty(option.node);
        }
    }
    parsed.document.mark_selectedness_normalized(select);
}

/// Handles a change to an option's `selected` attribute: with dirtiness false
/// the attribute drives selectedness, then the owning select resets
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#attr-option-selected>).
pub(crate) fn option_selected_attribute_changed(
    parsed: &mut Parsed,
    option: BlitzId,
    selected: bool,
) {
    if parsed.document.option_dirty(option) {
        return;
    }
    if let Some(select) = option_select_owner(&parsed.document.base, option)
        && selected
        && attr(&parsed.document.base, select, "multiple").is_none()
    {
        clear_other_options(parsed, select, option);
    }
    set_option_selectedness(parsed, option, selected);
    if let Some(select) = option_select_owner(&parsed.document.base, option) {
        parsed.document.invalidate_selectedness(select);
        run_selectedness_setting(parsed, select);
    }
}

/// Runs the selectedness rules after a select's `multiple`, `size`, or an
/// option's `disabled` attribute changed
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element>).
pub(crate) fn select_attribute_changed(parsed: &mut Parsed, select: BlitzId, attribute: &str) {
    if attribute == "multiple" && attr(&parsed.document.base, select, "multiple").is_none() {
        let mut seen = false;
        for option in select_options(&parsed.document.base, parsed.id, select) {
            if option_selectedness(parsed, option.node) {
                if seen {
                    set_option_selectedness(parsed, option.node, false);
                } else {
                    seen = true;
                }
            }
        }
    }
    parsed.document.invalidate_selectedness(select);
    run_selectedness_setting(parsed, select);
}
