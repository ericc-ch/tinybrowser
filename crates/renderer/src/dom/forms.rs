//! Form-control state read from the tree.
//!
//! Blitz keeps no form-control value store (no dirty value flag, checkedness
//! or selectedness slots), so every value here is read from content
//! attributes and descendant text. Option and select helpers live here once:
//! the IDL getters, the collections, and the insertion fixups share them
//! (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html>).

use blitz_dom::{BaseDocument, NodeData};

use crate::Parsed;
use crate::dom::text::descendant_text;
use crate::dom::tree::{BlitzId, NodeId, attr, form_owner_of, html_namespace, is_html_element};

/// A control's contribution to the entry list, resolved inside one world
/// borrow; file controls read their JS `files` after the borrow ends.
pub(crate) enum PendingEntry {
    Text(String, String),
    Files(String, NodeId),
}

/// Collects a form's entry list: every submittable control whose form owner is
/// the form, in tree order, that carries a name and is not disabled or inside a
/// `datalist`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set>).
pub(crate) fn collect_pending_entries(
    parsed: &Parsed,
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
        let id = NodeId {
            document,
            node: current,
        };
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
            && attr(base, current, "type")
                .is_some_and(|typ| typ.trim().eq_ignore_ascii_case("hidden"))
            && control_name.eq_ignore_ascii_case("_charset_")
        {
            pending.push(PendingEntry::Text(
                control_name.to_owned(),
                "UTF-8".to_owned(),
            ));
            continue;
        }
        let is_submitter = submitter == Some(id);
        pending.extend(pending_entry(
            base,
            document,
            current,
            local,
            control_name,
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
    base: &BaseDocument,
    document: u32,
    node: BlitzId,
    local: &str,
    control_name: &str,
    is_submitter: bool,
) -> Vec<PendingEntry> {
    let id = NodeId { document, node };
    // Blitz keeps no live control values, so the submission value is the
    // `value` content attribute (the default value), or empty when absent.
    let value_attr = || attr(base, node, "value").unwrap_or_default().to_owned();
    match local {
        "textarea" => {
            let mut value = normalize_newlines(&descendant_text(base, node).to_string_lossy());
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
            let typ = attr(base, node, "type").unwrap_or("text");
            let typ = typ.trim().to_ascii_lowercase();
            if matches!(typ.as_str(), "reset" | "button") {
                return Vec::new();
            }
            if matches!(typ.as_str(), "submit" | "image") {
                // A submit or image button contributes only when it is the
                // submitter.
                if !is_submitter {
                    return Vec::new();
                }
                return vec![PendingEntry::Text(control_name.to_owned(), value_attr())];
            }
            if typ == "checkbox" || typ == "radio" {
                // A checkbox or radio contributes only when checked, and its
                // value defaults to "on". Blitz keeps no dirty checkedness
                // flag, so checkedness is the `checked` attribute's presence
                // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value-default-on>).
                if attr(base, node, "checked").is_none() {
                    return Vec::new();
                }
                let value = attr(base, node, "value").unwrap_or("on").to_owned();
                vec![PendingEntry::Text(control_name.to_owned(), value)]
            } else if typ == "file" {
                vec![PendingEntry::Files(control_name.to_owned(), id)]
            } else {
                vec![PendingEntry::Text(control_name.to_owned(), value_attr())]
            }
        }
        "select" => {
            if attr(base, node, "multiple").is_some() {
                select_options(base, document, node)
                    .into_iter()
                    .filter(|option| {
                        option_selected(base, option.node) && !is_disabled(base, option.node)
                    })
                    .map(|option| {
                        PendingEntry::Text(control_name.to_owned(), option_value(base, option.node))
                    })
                    .collect()
            } else {
                vec![PendingEntry::Text(
                    control_name.to_owned(),
                    select_value(base, document, node),
                )]
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

/// An `option`'s selectedness: whether its `selected` attribute is present.
/// The dirty selectedness flag is a known gap; Blitz stores no per-option
/// state
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected>).
pub(crate) fn option_selected(base: &BaseDocument, option: BlitzId) -> bool {
    attr(base, option, "selected").is_some()
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
/// string. The parser-time ask (selecting the first option when none is
/// selected) is a known gap; without stored selectedness nothing selects it
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value>).
pub(crate) fn select_value(base: &BaseDocument, document: u32, select: BlitzId) -> String {
    for option in select_options(base, document, select) {
        if option_selected(base, option.node) {
            return option_value(base, option.node);
        }
    }
    String::new()
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

/// An API value with CRLF and CR newlines normalized to LF
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-fe-api-value>).
pub(crate) fn normalize_newlines(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    normalized
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
pub(crate) fn control_value_len(base: &blitz_dom::BaseDocument, node: BlitzId) -> u32 {
    let Some(element) = base
        .get_node(node)
        .and_then(|target| target.data.downcast_element())
    else {
        return 0;
    };
    if element.name.ns == html_namespace() && element.name.local.as_str() == "textarea" {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "control values stay far below u32::MAX"
        )]
        let len = crate::dom::text::descendant_text(base, node).units().len() as u32;
        return len;
    }
    let len = element
        .attr(markup5ever::LocalName::from("value"))
        .unwrap_or_default()
        .encode_utf16()
        .count();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "control values stay far below u32::MAX"
    )]
    let len = len as u32;
    len
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
        "input" => {
            // Only the text-like states support the selection APIs
            // (<https://html.spec.whatwg.org/multipage/input.html#do-not-apply>).
            let typ = attr(base, node, "type")
                .unwrap_or("text")
                .trim()
                .to_ascii_lowercase();
            matches!(
                typ.as_str(),
                "" | "text" | "search" | "tel" | "url" | "password"
            )
        }
        _ => false,
    }
}

/// Options to deselect when selected `option` joins a single-select: the
/// owner's other selected options. Returns empty when the option is not
/// selected, has no owner, or the owner allows multiple
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected>).
pub(crate) fn options_to_deselect(
    base: &BaseDocument,
    document: u32,
    option: BlitzId,
) -> Vec<NodeId> {
    let Some(element) = base
        .get_node(option)
        .and_then(|node| node.data.downcast_element())
    else {
        return Vec::new();
    };
    if element.name.ns != html_namespace() || element.name.local.as_str() != "option" {
        return Vec::new();
    }
    if attr(base, option, "selected").is_none() {
        return Vec::new();
    }
    let id = NodeId {
        document,
        node: option,
    };
    let Some(owner) = option_select_owner(base, option) else {
        return Vec::new();
    };
    let owner = NodeId {
        document,
        node: owner,
    };
    if attr(base, owner.node, "multiple").is_some() {
        return Vec::new();
    }
    select_options(base, document, owner.node)
        .into_iter()
        .filter(|other| *other != id && attr(base, other.node, "selected").is_some())
        .collect()
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
