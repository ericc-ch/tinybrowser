//! Constructing a form's entry list for `FormData` and form submission.
//!
//! [Constructing the entry list](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set)
//! walks a form's controls in tree order, keeping the ones that carry a name
//! and contribute a value. The list is returned to the `FormData` shim as a
//! flat `[name, value, ...]` array.
//!
//! Blitz keeps no form-control value store (no dirty value flag, no
//! checkedness or selectedness slots), so every value here is read from
//! content attributes and descendant text: an input's value is its `value`
//! attribute, checkedness is the presence of `checked`, a select's value
//! comes from the selected option's `value` attribute or text, and a
//! textarea's value is its descendant text. Live values assigned through the
//! `value` IDL setter are a known cutover gap until the control state moves
//! into the tree.

use super::{descendant_text, host_node_id, world, world_for_node};
use crate::js::world::{
    BlitzId, JournalEntry, NodeId, attr, form_owner_of, html_namespace, is_html_element,
};
use crate::js::{FrameNavigation, World};
use blitz_dom::{BaseDocument, NodeData};
use markup5ever::{LocalName, Namespace, QualName};
use rquickjs::prelude::Opt;
use rquickjs::{Array, Ctx, Object, Persistent, Result, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// Host functions the `FormData` and form-submission shims call.
pub(super) fn install(_ctx: &Ctx<'_>, globals: &Object<'_>) -> Result<()> {
    globals.set(
        "__tbFormEntries",
        rquickjs::prelude::Func::from(form_entries),
    )?;
    globals.set(
        "__tbFormNavigate",
        rquickjs::prelude::Func::from(form_navigate),
    )?;
    globals.set(
        "__tbSetInputFiles",
        rquickjs::prelude::Func::from(set_input_files),
    )?;
    globals.set("__tbEncodeForm", rquickjs::prelude::Func::from(encode_form))?;
    globals.set(
        "__tbUrlEncodeForm",
        rquickjs::prelude::Func::from(urlencode_form),
    )?;
    globals.set(
        "__tbEncodingName",
        rquickjs::prelude::Func::from(encoding_name),
    )?;
    globals.set(
        "__tbSetOptionSelectedness",
        rquickjs::prelude::Func::from(set_option_selectedness),
    )?;
    Ok(())
}

/// Strips ASCII whitespace, the only whitespace the Encoding Standard's
/// label matching removes
/// (<https://encoding.spec.whatwg.org/#concept-encoding-get>).
pub(crate) fn trim_label(label: &str) -> &str {
    label.trim_matches(['\t', '\n', '\x0C', '\r', ' '])
}

/// Resolves an encoding label to its canonical name, or `null`
/// (<https://encoding.spec.whatwg.org/#names-and-labels>).
///
/// The `replacement` encoding is never returned: getting an encoding
/// rejects it (<https://encoding.spec.whatwg.org/#concept-encoding-get>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn encoding_name(label: String) -> Option<String> {
    encoding_rs::Encoding::for_label(trim_label(&label).as_bytes())
        .filter(|encoding| *encoding != encoding_rs::REPLACEMENT)
        .map(|encoding| encoding.name().to_owned())
}

/// Sets an option's selectedness without the dirty flag, as the `Option`
/// constructor does. Blitz models no selectedness slot, so the `selected`
/// content attribute carries it directly.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn set_option_selectedness<'js>(ctx: Ctx<'js>, element: Value<'js>, selected: bool) -> Result<()> {
    let Some(node) = host_node_id(&ctx, &element) else {
        return Ok(());
    };
    let world = world_for_node(&ctx, node)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(node) else {
        return Ok(());
    };
    let (name, old_value) = {
        let base = &parsed.document.base;
        let Some(target) = base.get_node(node.node) else {
            return Ok(());
        };
        let Some(element) = target.data.downcast_element() else {
            return Ok(());
        };
        // Reuse the stored qualified name so clearing removes the exact
        // attribute the parser kept; a fresh `selected` falls back to the
        // empty namespace plain HTML attributes parse into.
        let name = element
            .attrs
            .iter()
            .find(|attribute| attribute.name.local.as_ref() == "selected")
            .map_or_else(
                || QualName::new(None, Namespace::from(""), LocalName::from("selected")),
                |attribute| attribute.name.clone(),
            );
        (name, attr(base, node.node, "selected").map(str::to_owned))
    };
    if selected == old_value.is_some() {
        return Ok(());
    }
    if selected {
        parsed
            .document
            .base
            .mutate()
            .set_attribute(node.node, name, "");
    } else {
        parsed
            .document
            .base
            .mutate()
            .clear_attribute(node.node, name);
    }
    parsed.document.record(JournalEntry::Attributes {
        target: node,
        name: "selected".to_owned(),
        namespace: String::new(),
        old_value,
    });
    Ok(())
}

/// A control's contribution to the entry list, resolved inside one world
/// borrow; file controls read their JS `files` after the borrow ends.
enum PendingEntry {
    Text(String, String),
    Files(String, NodeId),
}

/// The form's entry list as `[name, value, ...]`, in tree order.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn form_entries<'js>(
    ctx: Ctx<'js>,
    form: Value<'js>,
    submitter: Opt<Value<'js>>,
) -> Result<Array<'js>> {
    let entries = Array::new(ctx.clone())?;
    let Some(id) = host_node_id(&ctx, &form) else {
        return Ok(entries);
    };
    let world_rc = world(&ctx)?;
    let is_form = world_rc
        .borrow()
        .document(id)
        .is_some_and(|parsed| is_html_element(&parsed.document.base, id.node, "form"));
    if !is_form {
        return Ok(entries);
    }
    let submitter = submitter.0.and_then(|value| host_node_id(&ctx, &value));
    let pending = collect_pending_entries(&world_rc, id, submitter);
    for entry in pending {
        match entry {
            PendingEntry::Text(name, value) => {
                let index = entries.len();
                entries.set(index, name)?;
                entries.set(index + 1, value)?;
            }
            PendingEntry::Files(name, node) => {
                push_file_entries(&ctx, &entries, &world_rc, node, &name)?;
            }
        }
    }
    Ok(entries)
}

/// Collects a form's entry list: every submittable control whose form owner is
/// the form, in tree order, that carries a name and is not disabled or inside a
/// `datalist`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set>).
fn collect_pending_entries(
    world_rc: &Rc<RefCell<World>>,
    form: NodeId,
    submitter: Option<NodeId>,
) -> Vec<PendingEntry> {
    let world = world_rc.borrow();
    let Some(parsed) = world.document(form) else {
        return Vec::new();
    };
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
        let local = element.name.local.as_ref();
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
fn is_disabled(base: &BaseDocument, node: BlitzId) -> bool {
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
        match element.name.local.as_ref() {
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
fn has_datalist_ancestor(base: &BaseDocument, node: BlitzId) -> bool {
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
fn pending_entry(
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
/// descendant options whose nearest ancestor `select` is this one, so options
/// of a nested select belong to the inner select
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-option-list>).
fn select_options(base: &BaseDocument, document: u32, select: BlitzId) -> Vec<NodeId> {
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
            element.name.ns == html_namespace() && element.name.local.as_ref() == "option"
        }) && option_select_owner(base, current) == Some(select)
        {
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
fn option_select_owner(base: &BaseDocument, node: BlitzId) -> Option<BlitzId> {
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
fn option_selected(base: &BaseDocument, option: BlitzId) -> bool {
    attr(base, option, "selected").is_some()
}

/// An `option`'s value: its `value` attribute, else its text
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-value>).
fn option_value(base: &BaseDocument, option: BlitzId) -> String {
    attr(base, option, "value").map_or_else(|| option_text(base, option), str::to_owned)
}

/// An `option`'s text: its descendant text with ASCII whitespace stripped and
/// collapsed, skipping HTML and SVG `script` subtrees
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text>).
fn option_text(base: &BaseDocument, option: BlitzId) -> String {
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
                let is_script = element.name.local.as_ref().eq_ignore_ascii_case("script")
                    && (element.name.ns == html_namespace()
                        || element.name.ns.as_ref() == "http://www.w3.org/2000/svg");
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
fn select_value(base: &BaseDocument, document: u32, select: BlitzId) -> String {
    for option in select_options(base, document, select) {
        if option_selected(base, option.node) {
            return option_value(base, option.node);
        }
    }
    String::new()
}

/// Strips and collapses ASCII whitespace runs to single spaces, dropping
/// leading and trailing runs.
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

/// An API value with CRLF and CR newlines normalized to LF
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#concept-fe-api-value>).
fn normalize_newlines(value: &str) -> String {
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

/// Appends one `[name, File]` pair per file a script assigned to a `type=file`
/// input, read from the world so it does not depend on JS wrapper identity
/// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-files>).
fn push_file_entries<'js>(
    ctx: &Ctx<'js>,
    entries: &Array<'js>,
    world: &Rc<RefCell<World>>,
    node: NodeId,
    name: &str,
) -> Result<()> {
    let files: Vec<Value<'js>> = {
        let world = world.borrow();
        world
            .input_files(node)
            .map(|saved| {
                saved
                    .iter()
                    .filter_map(|saved| saved.clone().restore(ctx).ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    let no_files = files.is_empty();
    for file in files {
        let at = entries.len();
        entries.set(at, name)?;
        entries.set(at + 1, file)?;
    }
    if no_files
        && let Ok(empty) = crate::js::bridge::object(ctx)?.get::<_, Value>("__tbEmptyFile")
        && let Some(function) = empty.as_function()
        && let Ok(file) = function.call::<_, Value>(())
    {
        let at = entries.len();
        entries.set(at, name)?;
        entries.set(at + 1, file)?;
    }
    Ok(())
}

/// The submission encoding for a resolved label: the same ASCII-trimmed,
/// `replacement`-rejecting resolution as every other label entry, falling
/// back to UTF-8 because the caller passes an already-resolved name.
fn submission_encoding(label: &str) -> &'static encoding_rs::Encoding {
    encoding_rs::Encoding::for_label(trim_label(label).as_bytes())
        .filter(|encoding| *encoding != encoding_rs::REPLACEMENT)
        .unwrap_or(encoding_rs::UTF_8)
}

/// Encodes `text` in the form's submission encoding, returning a Latin-1
/// string of the encoded bytes. A character the encoding cannot represent
/// becomes a numeric character reference, per the Encoding Standard's
/// `encode` (<https://encoding.spec.whatwg.org/#encode>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn encode_form(text: String, label: String) -> String {
    let (bytes, _, _) = submission_encoding(&label).encode(&text);
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// Percent-encodes `text` for form submission: Encoding Standard `encode`
/// in `label`, then the urlencoded byte serializer over those bytes
/// (<https://url.spec.whatwg.org/#concept-urlencoded-byte-serializer>).
pub(super) fn urlencode_form(text: String, label: String) -> String {
    let (bytes, _, _) = submission_encoding(&label).encode(&text);
    url::form_urlencoded::byte_serialize(bytes.as_ref()).collect()
}

/// The host half of the `input.files` setter: records the assigned file list so
/// the form entry list can read it later.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn set_input_files<'js>(
    ctx: Ctx<'js>,
    element: Value<'js>,
    files: Value<'js>,
) -> Result<()> {
    let Some(node) = host_node_id(&ctx, &element) else {
        return Ok(());
    };
    let world_rc = world(&ctx)?;
    let mut stored = Vec::new();
    if let Some(object) = files.as_object() {
        let length = object.get::<_, f64>("length").unwrap_or(0.0);
        if length.is_finite() && length > 0.0 {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a file count is a small non-negative integer"
            )]
            let length = (length as usize).min(1024);
            for index in 0..length {
                let Ok(file) = object.get::<_, Value>(index.to_string()) else {
                    continue;
                };
                if !file.is_undefined() && !file.is_null() {
                    stored.push(Persistent::save(&ctx, file));
                }
            }
        }
    }
    world_rc.borrow_mut().set_input_files(node, stored);
    Ok(())
}

/// Whether a `textarea`'s `wrap` attribute is in the Hard state
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#attr-textarea-wrap>):
/// an enumerated attribute, ASCII case-insensitive.
fn wrap_is_hard(wrap: Option<&str>) -> bool {
    wrap.is_some_and(|wrap| wrap.trim().eq_ignore_ascii_case("hard"))
}

/// The non-negative integer in `value`, or `None` for an absent or invalid
/// attribute
/// (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-non-negative-integers>).
fn parse_positive(value: Option<&str>) -> Option<usize> {
    let parsed = super::parse_non_negative_integer(value?)?;
    Some(usize::try_from(parsed).unwrap_or(usize::MAX))
}

/// The textarea hard-wrapping transformation: break each line at `cols`
/// characters and join with LF. The submitted value normalizes to LF here; the
/// urlencoded serializer turns those into CRLF pairs
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#textarea-wrapping-transformation>).
fn hard_wrap(value: &str, cols: usize) -> String {
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

/// Queues a form-submission navigation to `url`. A `target` naming an `iframe`
/// lands in that frame; `_self`, `_top`, `_parent`, `_blank`, and the empty
/// target all land in the submitting frame (a new tab for `_blank` is not
/// modelled yet)
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission-algorithm>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn form_navigate(
    ctx: Ctx<'_>,
    url: String,
    target: String,
    method: String,
    body: String,
    content_type: Option<String>,
) -> Result<()> {
    let world_rc = world(&ctx)?;
    let container = if target.is_empty() || target.starts_with('_') {
        None
    } else {
        find_named_frame(&world_rc, &target)
    };
    let navigation_target = match container {
        Some(container) => crate::js::NavigationTarget::Container(container),
        None => crate::js::NavigationTarget::SelfFrame,
    };
    let navigation = FrameNavigation {
        target: navigation_target,
        spec: url,
        method,
        // The body arrives as a Latin-1 string, one char per byte, so file
        // bytes survive the JS boundary unchanged
        // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#multipart-form-data>).
        body: body
            .chars()
            .map(|character| {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "the JS side maps each byte to U+0000..U+00FF"
                )]
                let byte = character as u8;
                byte
            })
            .collect(),
        content_type,
    };
    world_rc.borrow_mut().queue_frame_navigation(navigation);
    Ok(())
}

/// The first `iframe` in the main document whose `name` matches, for a form
/// `target` that is a browsing-context name.
fn find_named_frame(world: &Rc<RefCell<World>>, name: &str) -> Option<NodeId> {
    let world = world.borrow();
    let parsed = world.main_document()?;
    let base = &parsed.document.base;
    let document = parsed.id;
    let mut stack = vec![base.root_node().id];
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        if is_html_element(base, current, "iframe")
            && attr(base, current, "name").is_some_and(|value| value == name)
        {
            return Some(NodeId {
                document,
                node: current,
            });
        }
        stack.extend(node.children.iter().rev().copied());
    }
    None
}
