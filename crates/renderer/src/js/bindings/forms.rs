//! Constructing a form's entry list for `FormData` and form submission.
//!
//! [Constructing the entry list](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set)
//! walks a form's controls in tree order, keeping the ones that carry a name
//! and contribute a value. The list is returned to the `FormData` shim as a
//! flat `[name, value, ...]` array.

use super::{host_node_id, is_html_element, with_node_kind, world};
use dom::{NodeId, NodeKind, html_namespace};
use rquickjs::{Ctx, Result, Value};

/// The form's entry list as `[name, value, ...]`, in tree order.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn form_entries<'js>(ctx: Ctx<'js>, form: Value<'js>) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    let Some(id) = host_node_id(&ctx, &form) else {
        return Ok(entries);
    };
    if !with_node_kind(&ctx, id, |kind| is_html_element(kind, "form"))? {
        return Ok(entries);
    }
    let world_rc = world(&ctx)?;
    let world = world_rc.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(entries);
    };
    // Pre-order traversal: an explicit stack with reversed children keeps
    // document order.
    let mut order = Vec::new();
    let mut stack = vec![id];
    while let Some(node) = stack.pop() {
        order.push(node);
        let children: Vec<NodeId> = parsed
            .dom
            .children(node)
            .map(Iterator::collect)
            .unwrap_or_default();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    for node in order {
        if node == id {
            continue;
        }
        let Some(NodeKind::Element { name, .. }) = parsed.dom.kind(node) else {
            continue;
        };
        if name.ns != html_namespace() {
            continue;
        }
        let local = name.local.as_ref();
        if local != "input" && local != "textarea" && local != "select" {
            continue;
        }
        // A control without a name, or disabled, contributes nothing
        // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set>).
        let Some(control_name) = parsed.dom.attribute(node, "name") else {
            continue;
        };
        if control_name.is_empty() || parsed.dom.attribute(node, "disabled").is_some() {
            continue;
        }
        match local {
            "textarea" => {
                let mut value = parsed.dom.textarea_value(node).unwrap_or_default();
                if wrap_is_hard(parsed.dom.attribute(node, "wrap").as_deref()) {
                    let cols = parse_positive(parsed.dom.attribute(node, "cols").as_deref())
                        .unwrap_or(20);
                    value = hard_wrap(&value, cols);
                }
                entries.push(control_name);
                entries.push(value);
            }
            "input" => {
                // Only text-entry inputs for now; checkbox/radio/select need
                // checkedness/selectedness state that lands with the input
                // units.
                let typ = parsed
                    .dom
                    .attribute(node, "type")
                    .unwrap_or_else(|| "text".to_owned());
                let typ = typ.trim().to_ascii_lowercase();
                if matches!(
                    typ.as_str(),
                    "submit"
                        | "reset"
                        | "button"
                        | "image"
                        | "checkbox"
                        | "radio"
                        | "file"
                        | "hidden"
                ) {
                    continue;
                }
                entries.push(control_name);
                entries.push(parsed.dom.input_value(node).unwrap_or_default());
            }
            _ => {}
        }
    }
    Ok(entries)
}

/// Whether a `textarea`'s `wrap` attribute is in the Hard state
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#attr-textarea-wrap>):
/// an enumerated attribute, ASCII case-insensitive.
fn wrap_is_hard(wrap: Option<&str>) -> bool {
    wrap.is_some_and(|wrap| wrap.trim().eq_ignore_ascii_case("hard"))
}

/// The non-negative integer in `value`, or `None` for an absent or invalid
/// attribute (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-non-negative-integers>).
fn parse_positive(value: Option<&str>) -> Option<usize> {
    let text = value?.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
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
