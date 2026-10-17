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

use super::{focus, host_node_id, owning_world, world, world_for_node};
use crate::documents::JournalEntry;
use crate::dom::forms::{PendingEntry, encode_form, trim_label, urlencode_form};
use crate::dom::tree::{NodeId, attr, is_html_element};
use crate::js::{FrameNavigation, World};
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
    globals.set(
        "__tbEncodeForm",
        rquickjs::prelude::Func::from(|text: String, label: String| encode_form(&text, &label)),
    )?;
    globals.set(
        "__tbUrlEncodeForm",
        rquickjs::prelude::Func::from(|text: String, label: String| urlencode_form(&text, &label)),
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
            .find(|attribute| attribute.name.local.as_str() == "selected")
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
    let pending = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(id) else {
            return Ok(entries);
        };
        crate::dom::forms::collect_pending_entries(&parsed, id, submitter)
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

/// Queues a form-submission navigation to `url`, to the navigable the form's
/// `target` chooses
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission-algorithm>).
///
/// Submission returns without navigating when the form cannot navigate
/// (<https://html.spec.whatwg.org/multipage/links.html#cannot-navigate>), which
/// is why the form itself crosses the boundary.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(super) fn form_navigate<'js>(
    ctx: Ctx<'js>,
    url: String,
    target: String,
    method: String,
    body: String,
    content_type: Option<String>,
    form: Value<'js>,
) -> Result<()> {
    let Some(node) = host_node_id(&ctx, &form) else {
        return Ok(());
    };
    // A form whose document is gone cannot navigate either, and must not throw
    // on the way to that conclusion.
    let Some(owner) = owning_world(&ctx, node) else {
        return Ok(());
    };
    if focus::cannot_navigate(&owner.borrow(), node) {
        return Ok(());
    }
    let world_rc = world(&ctx)?;
    let navigation = FrameNavigation {
        target: crate::js::NavigationTarget::Target(target),
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
        // A form submission carries no policy of its own yet, so the initiator
        // document's referrer policy decides.
        referrer_policy: String::new(),
    };
    world_rc.borrow_mut().queue_frame_navigation(navigation);
    Ok(())
}
