//! Constructing a form's entry list for `FormData` and form submission.
//!
//! [Constructing the entry list](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set)
//! walks a form's controls in tree order, keeping the ones that carry a name
//! and contribute a value. The list is returned to the `FormData` shim as a
//! flat `[name, value, ...]` array.

use super::{host_node_id, is_html_element, with_node_kind, world};
use crate::js::{FrameNavigation, World};
use dom::{NodeId, NodeKind, html_namespace};
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
pub(super) fn form_entries<'js>(ctx: Ctx<'js>, form: Value<'js>) -> Result<Array<'js>> {
    let entries = Array::new(ctx.clone())?;
    let Some(id) = host_node_id(&ctx, &form) else {
        return Ok(entries);
    };
    if !with_node_kind(&ctx, id, |kind| is_html_element(kind, "form"))? {
        return Ok(entries);
    }
    let world_rc = world(&ctx)?;
    let pending: Vec<PendingEntry> = {
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
        let mut pending = Vec::new();
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
                    pending.push(PendingEntry::Text(control_name, value));
                }
                "input" => {
                    let typ = parsed
                        .dom
                        .attribute(node, "type")
                        .unwrap_or_else(|| "text".to_owned());
                    let typ = typ.trim().to_ascii_lowercase();
                    if matches!(
                        typ.as_str(),
                        "submit" | "reset" | "button" | "image" | "checkbox" | "radio"
                    ) {
                        continue;
                    }
                    if typ == "file" {
                        pending.push(PendingEntry::Files(control_name, node));
                    } else {
                        pending.push(PendingEntry::Text(
                            control_name,
                            parsed.dom.input_value(node).unwrap_or_default(),
                        ));
                    }
                }
                // A `select` contributes through selectedness, which lands
                // with the select unit.
                _ => {}
            }
        }
        pending
    };
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
    for file in files {
        let at = entries.len();
        entries.set(at, name)?;
        entries.set(at + 1, file)?;
    }
    Ok(())
}

/// The host half of the `input.files` setter: records the assigned file list so
/// the form entry list can read it later.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn set_input_files<'js>(    ctx: Ctx<'js>,
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
        body: body.into_bytes(),
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
    let mut stack = vec![parsed.dom.document()];
    while let Some(node) = stack.pop() {
        if let Some(NodeKind::Element { name: element, .. }) = parsed.dom.kind(node)
            && element.ns == html_namespace()
            && element.local.as_ref() == "iframe"
            && parsed.dom.attribute(node, "name").as_deref() == Some(name)
        {
            return Some(node);
        }
        let children: Vec<NodeId> = parsed
            .dom
            .children(node)
            .map(Iterator::collect)
            .unwrap_or_default();
        stack.extend(children);
    }
    None
}
