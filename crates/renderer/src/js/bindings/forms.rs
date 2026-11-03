//! Constructing a form's entry list for `FormData` and form submission.
//!
//! [Constructing the entry list](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set)
//! walks a form's controls in tree order, keeping the ones that carry a name
//! and contribute a value. The list is returned to the `FormData` shim as a
//! flat `[name, value, ...]` array.
//!
//! Values come from the form-control slots in the tree: an input's current
//! value, an option's selectedness, and a textarea's dirty value or descendant
//! text. The dirty flags decide whether a content attribute still drives a
//! slot (<https://html.spec.whatwg.org/multipage/input.html#concept-fe-value>,
//! <https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-selectedness>).

use super::{focus, host_node_id, owning_world, world, world_for_node};
use crate::dom::forms::{self, PendingEntry, encode_form, trim_label, urlencode_form};
use crate::dom::input::{
    InputType, NumberBufferState, number_buffer_state, step_mismatch, value_as_number,
    value_in_units,
};
use crate::dom::tree::{NodeId, is_html_element};
use crate::js::{FrameNavigation, World};
use rquickjs::prelude::Opt;
use rquickjs::{Array, Ctx, Exception, IntoJs, Object, Persistent, Result, TypedArray, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// A host result where `None` is JS `null`. rquickjs maps a bare `Option::None`
/// to `undefined`, but the shims use `null` for "no value" and compare with
/// `=== null`.
fn optional<'js, T: IntoJs<'js>>(value: Option<T>, ctx: &Ctx<'js>) -> Result<Value<'js>> {
    match value {
        Some(value) => value.into_js(ctx),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

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
    globals.set("__tbFormPlan", rquickjs::prelude::Func::from(form_plan))?;
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
        rquickjs::prelude::Func::from(store_option_selectedness),
    )?;
    globals.set(
        "__tbFormCanSubmit",
        rquickjs::prelude::Func::from(form_can_submit),
    )?;
    globals.set(
        "__tbSameOriginWithTop",
        rquickjs::prelude::Func::from(same_origin_with_top),
    )?;
    globals.set("__tbParseFloat", rquickjs::prelude::Func::from(parse_float))?;
    globals.set(
        "__tbParseNonNegativeInteger",
        rquickjs::prelude::Func::from(parse_non_negative_integer_js),
    )?;
    globals.set(
        "__tbValueInUnits",
        rquickjs::prelude::Func::from(value_in_units_js),
    )?;
    globals.set(
        "__tbValueAsNumber",
        rquickjs::prelude::Func::from(value_as_number_js),
    )?;
    globals.set(
        "__tbStepMismatch",
        rquickjs::prelude::Func::from(step_mismatch_js),
    )?;
    globals.set(
        "__tbDecimalScale",
        rquickjs::prelude::Func::from(decimal_scale_js),
    )?;
    globals.set(
        "__tbDefaultStep",
        rquickjs::prelude::Func::from(default_step_js),
    )?;
    globals.set(
        "__tbNumberBufferState",
        rquickjs::prelude::Func::from(number_buffer_state_js),
    )?;
    Ok(())
}

/// The floating-point grammar over an owned string, with `null` for a parse
/// failure.
fn parse_float(ctx: Ctx<'_>, value: String) -> Result<Value<'_>> {
    optional(crate::dom::parse::parse_double(&value), &ctx)
}

/// The non-negative integer rules over an owned string, with `null` for a parse
/// failure.
fn parse_non_negative_integer_js(ctx: Ctx<'_>, value: String) -> Result<Value<'_>> {
    optional(crate::dom::parse::parse_non_negative_integer(&value), &ctx)
}

/// The authored decimal scale of a step literal, or `null` when it is not one.
fn decimal_scale_js(ctx: Ctx<'_>, value: String) -> Result<Value<'_>> {
    optional(
        crate::dom::microsyntaxes::authored_decimal_scale(&value),
        &ctx,
    )
}

/// The state's default step in its step unit, or `null` when the state does
/// not step.
fn default_step_js(ctx: Ctx<'_>, type_name: String) -> Result<Value<'_>> {
    optional(
        crate::dom::input::default_step(InputType::from_attribute(Some(&type_name))),
        &ctx,
    )
}

/// The input state for a `type` keyword, as the IDL `type` attribute spells
/// it. An unknown keyword is the Text state, which has no value in any
/// stepping unit.
fn input_kind(type_name: &str) -> InputType {
    InputType::from_attribute(Some(type_name))
}

/// The value interpreted in its state's step unit, or `null` for an invalid
/// value or a state that does not step.
fn value_in_units_js(ctx: Ctx<'_>, type_name: String, text: String) -> Result<Value<'_>> {
    optional(value_in_units(input_kind(&type_name), &text), &ctx)
}

/// The value as `valueAsNumber` reports it, or `null` for an invalid value or
/// a state that has no numeric value.
fn value_as_number_js(ctx: Ctx<'_>, type_name: String, text: String) -> Result<Value<'_>> {
    optional(value_as_number(input_kind(&type_name), &text), &ctx)
}

/// Whether the input suffers from a step mismatch.
fn step_mismatch_js<'js>(ctx: Ctx<'js>, element: Value<'js>) -> Result<bool> {
    let Some(id) = host_node_id(&ctx, &element) else {
        return Ok(false);
    };
    let world = world(&ctx)?;
    let world = world.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(false);
    };
    Ok(step_mismatch(&parsed.document.base, id.node))
}

/// The number edit buffer state as `[0, 1, 2, 3, 4]` for invalid, empty,
/// partial, complete, and trailing-dot, in the order the JS shim reads them.
fn number_buffer_state_js(text: String) -> u8 {
    match number_buffer_state(&text) {
        NumberBufferState::Invalid => 0,
        NumberBufferState::Empty => 1,
        NumberBufferState::Partial => 2,
        NumberBufferState::Complete => 3,
        NumberBufferState::TrailingDot => 4,
    }
}

/// Whether the calling realm is same origin with its top-level traversable.
/// `showPicker()` requires it, and `document.domain` does not widen it
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-showpicker>).
fn same_origin_with_top(ctx: Ctx<'_>) -> Result<bool> {
    let current = world(&ctx)?;
    let frame = current.borrow().frame();
    let top = current
        .borrow()
        .runtime
        .shared
        .borrow()
        .tree
        .top_level(frame);
    if top == frame {
        return Ok(true);
    }
    let Some(top_world) = current.borrow().frame_world(top) else {
        return Ok(false);
    };
    let same = current.borrow().origin == top_world.borrow().origin;
    Ok(same)
}

/// Whether `form` can start a submission: a stale wrapper or a form outside
/// the main document has no browsing context to navigate
/// (<https://html.spec.whatwg.org/multipage/forms.html#dom-form-submit>).
fn form_can_submit<'js>(ctx: Ctx<'js>, form: Value<'js>) -> bool {
    let Some(node) = host_node_id(&ctx, &form) else {
        return false;
    };
    let Some(owner) = owning_world(&ctx, node) else {
        return false;
    };
    !focus::cannot_navigate(&owner.borrow(), node)
}

/// Resolves an encoding label to its canonical name, or `null`
/// (<https://encoding.spec.whatwg.org/#names-and-labels>).
///
/// The `replacement` encoding is never returned: getting an encoding
/// rejects it (<https://encoding.spec.whatwg.org/#concept-encoding-get>).
pub(super) fn encoding_name(label: String) -> Option<String> {
    encoding_rs::Encoding::for_label(trim_label(&label).as_bytes())
        .filter(|encoding| *encoding != encoding_rs::REPLACEMENT)
        .map(|encoding| encoding.name().to_owned())
}

/// Sets an option's selectedness without the dirtiness flag, as the
/// `Option()` constructor does, and leaves the `selected` content attribute
/// driving it
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-option>).
fn store_option_selectedness<'js>(
    ctx: Ctx<'js>,
    element: Value<'js>,
    selected: bool,
) -> Result<()> {
    let Some(node) = host_node_id(&ctx, &element) else {
        return Ok(());
    };
    let world = world_for_node(&ctx, node)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(node) else {
        return Ok(());
    };
    forms::set_option_selectedness(&mut parsed, node.node, selected);
    Ok(())
}

/// The form's entry list as `[name, value, ...]`, in tree order.
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
        let Some(mut parsed) = world.document_mut(id) else {
            return Ok(entries);
        };
        crate::dom::forms::collect_pending_entries(&mut parsed, id, submitter)
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

/// The resolved submission plan for a form and an optional submitter, as
/// `[action, method, enctype, target]`, or `null` when the form cannot submit
/// (no action that resolves, or `dialog`).
///
/// The renderer owns the event half (`submit`, `formdata`) and the encoders;
/// the method/action/enctype/target resolution and their parsers are the
/// engine's one copy
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission-algorithm>).
pub(super) fn form_plan<'js>(
    ctx: Ctx<'js>,
    form: Value<'js>,
    submitter: Opt<Value<'js>>,
) -> Result<Value<'js>> {
    let Some(id) = host_node_id(&ctx, &form) else {
        return Ok(Value::new_null(ctx));
    };
    let world = world(&ctx)?;
    let world = world.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(Value::new_null(ctx));
    };
    let submitter = submitter.0.and_then(|value| host_node_id(&ctx, &value));
    // A submitter from another document cannot override this form's attributes;
    // the node id alone would address an unrelated element in this tree.
    let submitter = submitter.filter(|submitter| submitter.document == id.document);
    let Some(plan) = parsed
        .document
        .base
        .plan_form_submission(id.node, submitter.map(|submitter| submitter.node))
    else {
        return Ok(Value::new_null(ctx.clone()));
    };
    let array = Array::new(ctx.clone())?;
    array.set(0, plan.action.to_string())?;
    array.set(1, plan.method.as_str())?;
    array.set(2, plan.enctype.as_str())?;
    array.set(3, plan.target.unwrap_or_default())?;
    Ok(array.into_value())
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
            #[expect(
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
pub(super) fn form_navigate<'js>(
    ctx: Ctx<'js>,
    url: String,
    target: String,
    method: String,
    body: TypedArray<'js, u8>,
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
    // A form submission is GET or POST; `method=dialog` never reaches the
    // host, and the shim normalizes every other spelling to GET
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fs-method>).
    let method = match method.as_str() {
        "GET" => net::Method::GET,
        "POST" => net::Method::POST,
        other => {
            return Err(Exception::throw_type(
                &ctx,
                &format!("form method: {other:?} is neither GET nor POST"),
            ));
        }
    };
    if focus::cannot_navigate(&owner.borrow(), node) {
        return Ok(());
    }
    let world_rc = world(&ctx)?;
    // A sandboxed document without `allow-forms` blocks form submission
    // (<https://html.spec.whatwg.org/multipage/browsers.html#sandboxed-forms-browsing-context-flag>).
    if world_rc
        .borrow()
        .sandbox
        .contains(crate::protocol::SandboxFlag::Forms)
    {
        return Ok(());
    }
    let mut navigation = FrameNavigation::get(
        crate::js::NavigationTarget::Target(crate::protocol::TargetName::parse(&target)),
        url,
    );
    navigation.method = method;
    navigation.body = body.to_vec();
    navigation.content_type = content_type;
    // A form submission carries no policy of its own yet, so the initiator
    // document's referrer policy decides.
    world_rc.borrow_mut().queue_frame_navigation(navigation);
    Ok(())
}
