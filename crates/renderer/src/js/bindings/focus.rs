//! Focus, activation behavior, and the `WebDriver` bridge.

use super::{
    LegacyNullString, events, host_node_id, webdriver_element, world_for_node,
    wrap_node,
};

use markup5ever::{LocalName, Namespace, QualName};
use rquickjs::{Class, Ctx, Exception, Function, Result, Value, prelude::This};

use crate::js::world::EventTargetKey;
use crate::js::world::{BlitzId, JournalEntry, NodeId, attr, html_namespace, is_connected};

/// The node-removal focus fixup: when the document's focused area is inside
/// a removed subtree, clear it without firing events
/// (<https://html.spec.whatwg.org/multipage/dom.html#node-remove-focus-fixup>).
pub(crate) fn fixup_focus_after_removal(ctx: &Ctx<'_>, removed: NodeId) -> Result<()> {
    let world = world_for_node(ctx, removed)?;
    let document = removed.document_id();
    let Some(active) = world.borrow().active_element(document) else {
        return Ok(());
    };
    let mut cursor = Some(active);
    while let Some(current) = cursor {
        if current == removed {
            world.borrow_mut().set_active_element(document, None);
            return Ok(());
        }
        cursor = world.borrow().node_parent(current);
    }
    Ok(())
}

/// Whether an element is a focusable area
/// (<https://html.spec.whatwg.org/multipage/interaction.html#focusable-area>).
///
/// The engine has no layout, so visibility and being rendered cannot be part
/// of the decision; what remains is the element, disabled, connection, and
/// known-focusable-locals core. Known cutover gaps: `contenteditable` editing
/// hosts, `area`/`iframe` shapes, SVG focusability beyond `tabindex`, and the
/// full `tabindex` value parsing (any presence counts here).
pub(crate) fn is_focusable(ctx: &Ctx<'_>, node: NodeId) -> Result<bool> {
    let world = world_for_node(ctx, node)?;
    let world = world.borrow();
    let Some(parsed) = world.document(node) else {
        return Ok(false);
    };
    let base = &parsed.document.base;
    let Some(tree) = base.get_node(node.node) else {
        return Ok(false);
    };
    let Some(element) = tree.data.downcast_element() else {
        return Ok(false);
    };
    if !is_connected(base, node.node) || is_actually_disabled(base, node.node) {
        return Ok(false);
    }
    // `tabindex` applies to SVG elements too.
    if attr(base, node.node, "tabindex").is_some() {
        return Ok(true);
    }
    if element.name.ns != html_namespace() {
        return Ok(false);
    }
    Ok(match element.name.local.as_ref() {
        "input" => !is_hidden_input(base, node.node),
        "a" => attr(base, node.node, "href").is_some(),
        "button" | "select" | "textarea" => true,
        _ => false,
    })
}

/// The HTML "actually disabled" check for the form controls the engine
/// supports, including descendants of a disabled `fieldset` that are not
/// inside its first `legend`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-fe-disabled>).
fn is_actually_disabled(base: &blitz_dom::BaseDocument, node: BlitzId) -> bool {
    if attr(base, node, "disabled").is_some()
        && matches!(
            node_local_name(base, node).as_deref(),
            Some("button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset")
        )
    {
        return true;
    }
    let mut cursor = base.get_node(node).and_then(|tree| tree.parent);
    while let Some(parent) = cursor {
        if node_local_name(base, parent).as_deref() == Some("fieldset")
            && attr(base, parent, "disabled").is_some()
        {
            let first_legend = base.get_node(parent).and_then(|tree| {
                tree.children
                    .iter()
                    .copied()
                    .find(|&child| node_local_name(base, child).as_deref() == Some("legend"))
            });
            if let Some(legend) = first_legend {
                let mut inner = Some(node);
                while let Some(current) = inner {
                    if current == legend {
                        return false;
                    }
                    inner = base.get_node(current).and_then(|tree| tree.parent);
                }
            }
            return true;
        }
        cursor = base.get_node(parent).and_then(|tree| tree.parent);
    }
    false
}

fn is_hidden_input(base: &blitz_dom::BaseDocument, node: BlitzId) -> bool {
    attr(base, node, "type").is_some_and(|kind| kind.eq_ignore_ascii_case("hidden"))
}

fn node_local_name(base: &blitz_dom::BaseDocument, node: BlitzId) -> Option<String> {
    base.get_node(node)
        .and_then(|tree| tree.data.downcast_element())
        .and_then(|element| {
            (element.name.ns == html_namespace()).then(|| element.name.local.to_string())
        })
}

/// Writes one content attribute (or removes it for `None`), reusing the
/// stored qualified name so clearing removes the exact attribute the parser
/// kept (see `forms.rs` for the same pattern).
fn write_attr(parsed: &mut crate::Parsed, node: NodeId, local: &str, value: Option<&str>) {
    let (name, old_value) = {
        let base = &parsed.document.base;
        let Some(tree) = base.get_node(node.node) else {
            return;
        };
        let Some(element) = tree.data.downcast_element() else {
            return;
        };
        let name = element
            .attrs
            .iter()
            .find(|attribute| attribute.name.local.as_ref() == local)
            .map_or_else(
                || QualName::new(None, Namespace::from(""), LocalName::from(local)),
                |attribute| attribute.name.clone(),
            );
        (name, attr(base, node.node, local).map(str::to_owned))
    };
    if old_value.as_deref() == value {
        return;
    }
    match value {
        Some(value) => {
            parsed
                .document
                .base
                .mutate()
                .set_attribute(node.node, name, value);
        }
        None => {
            parsed
                .document
                .base
                .mutate()
                .clear_attribute(node.node, name);
        }
    }
    parsed.document.record(JournalEntry::Attributes {
        target: node,
        name: local.to_owned(),
        namespace: String::new(),
        old_value,
    });
}

/// Sets or clears a boolean content attribute such as `checked`.
fn set_presence_attr(parsed: &mut crate::Parsed, node: NodeId, local: &str, present: bool) {
    write_attr(parsed, node, local, present.then_some(""));
}

/// The `type` of an `input`, lowercased and trimmed; missing means `text`.
fn input_type(base: &blitz_dom::BaseDocument, node: BlitzId) -> String {
    attr(base, node, "type")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// Radios sharing `node`'s `name` in the same tree, in tree order. Blitz
/// keeps no form-owner model, so the parser-associated form owner is a known
/// cutover gap and the group spans the document instead of the form.
fn radio_group(base: &blitz_dom::BaseDocument, document: u32, node: BlitzId) -> Vec<NodeId> {
    let name = attr(base, node, "name").unwrap_or_default().to_owned();
    let mut group = Vec::new();
    let mut stack = vec![base.root_node().id];
    // Reverse-push keeps the pop order in tree order.
    while let Some(id) = stack.pop() {
        let Some(tree) = base.get_node(id) else {
            continue;
        };
        if tree.data.downcast_element().is_some_and(|element| {
            element.name.ns == html_namespace()
                && element.name.local.as_ref() == "input"
                && input_type(base, id) == "radio"
                && attr(base, id, "name").unwrap_or_default() == name
        }) {
            group.push(NodeId { document, node: id });
        }
        stack.extend(tree.children.iter().rev().copied());
    }
    group.sort_by(|a, b| super::tree_order(base, document, a.node, b.node));
    group
}

/// The group's currently checked radio, if any.
fn radio_group_checked(
    base: &blitz_dom::BaseDocument,
    document: u32,
    node: BlitzId,
) -> Option<NodeId> {
    radio_group(base, document, node)
        .into_iter()
        .find(|member| attr(base, member.node, "checked").is_some())
}

/// The nearest ancestor `form` element. Blitz models no parser-associated
/// form owner (the `form` attribute and its scoping are known cutover gaps),
/// so controls outside a form ancestor have no owner here.
fn form_owner(base: &blitz_dom::BaseDocument, node: BlitzId) -> Option<BlitzId> {
    let mut cursor = base.get_node(node).and_then(|tree| tree.parent);
    while let Some(id) = cursor {
        let is_form = base
            .get_node(id)
            .and_then(|tree| tree.data.downcast_element())
            .is_some_and(|element| {
                element.name.ns == html_namespace() && element.name.local.as_ref() == "form"
            });
        if is_form {
            return Some(id);
        }
        cursor = base.get_node(id).and_then(|tree| tree.parent);
    }
    None
}

/// Moves focus to `node`. The previously focused area is cleared before the
/// `blur`/`focusout` chain, and the new one installed before `focus`/`focusin`
/// (<https://html.spec.whatwg.org/multipage/interaction.html#focus-update-steps>).
pub(crate) fn focus_node(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    let world = world_for_node(ctx, node)?;
    let document = node.document_id();
    let previous = world.borrow().active_element(document);
    if previous == Some(node) {
        return Ok(());
    }
    // `set_active_element` mirrors into Blitz (`set_focus_to`/`clear_focus`).
    // Focus events carry no `relatedTarget` until `FocusEvent` exists.
    world.borrow_mut().set_active_element(document, None);
    if let Some(previous) = previous {
        events::fire_trusted(ctx, EventTargetKey::Node(previous), "blur", false, false)?;
        // A handler may have moved focus; the spec's focus update steps stop
        // when the focused area changed during the blur chain.
        if world.borrow().active_element(document).is_some() {
            events::fire_trusted(
                ctx,
                EventTargetKey::Node(previous),
                "focusout",
                true,
                false,
            )?;
            return Ok(());
        }
        events::fire_trusted(
            ctx,
            EventTargetKey::Node(previous),
            "focusout",
            true,
            false,
        )?;
    }
    // Handlers may have made the target unfocusable; browsers then do not
    // designate or fire on it.
    if !is_focusable(ctx, node)? {
        return Ok(());
    }
    world.borrow_mut().set_active_element(document, Some(node));
    events::fire_trusted(ctx, EventTargetKey::Node(node), "focus", false, false)?;
    events::fire_trusted(ctx, EventTargetKey::Node(node), "focusin", true, false)?;
    Ok(())
}

/// Clears the focused area when `node` is it
/// (<https://html.spec.whatwg.org/multipage/interaction.html#dom-blur>).
pub(crate) fn blur_node(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    let world = world_for_node(ctx, node)?;
    let document = node.document_id();
    if world.borrow().active_element(document) != Some(node) {
        return Ok(());
    }
    world.borrow_mut().set_active_element(document, None);
    events::fire_trusted(ctx, EventTargetKey::Node(node), "blur", false, false)?;
    events::fire_trusted(ctx, EventTargetKey::Node(node), "focusout", true, false)?;
    Ok(())
}

/// Dispatches a synthetic (untrusted) `click`
/// (<https://html.spec.whatwg.org/multipage/interaction.html#dom-click>).
pub(crate) fn element_click(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    let world = world_for_node(ctx, node)?;
    {
        let world = world.borrow();
        let Some(parsed) = world.document(node) else {
            return Ok(());
        };
        if is_actually_disabled(&parsed.document.base, node.node)
            || world.click_in_progress(node)
        {
            return Ok(());
        }
    }
    world.borrow_mut().set_click_in_progress(node, true);
    let result = (|| {
        // `HTMLElement.click()` focuses a focusable target before dispatching
        // (<https://html.spec.whatwg.org/multipage/interaction.html#dom-click>).
        if is_focusable(ctx, node)? {
            focus_node(ctx, node)?;
        }
        // The legacy-pre-activation behavior updates checkedness before the
        // `click` event, so a handler observes the new state; a canceled click
        // restores it afterwards
        // (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:legacy-pre-activation-behavior>).
        let previous = legacy_pre_activation(ctx, node)?;
        let event = Class::instance(ctx.clone(), events::JsEvent::uninitialized())?;
        event.borrow().initialize("click".to_owned(), true, true);
        let not_canceled = events::dispatch_event(ctx, EventTargetKey::Node(node), &event)?;
        if not_canceled {
            complete_activation(ctx, node)?;
        } else {
            legacy_canceled_activation(ctx, node, &previous)?;
        }
        Ok(())
    })();
    world.borrow_mut().set_click_in_progress(node, false);
    result
}

/// The checkedness state a canceled click restores.
enum PreActivation {
    None,
    Checkbox { checked: bool },
    Radio(Option<NodeId>),
}

/// The legacy-pre-activation behavior: a checkbox toggles, a radio becomes
/// checked and remembers the group's previous checked radio. Blitz keeps no
/// checkedness slot, so the `checked` attribute is the whole state.
fn legacy_pre_activation(ctx: &Ctx<'_>, node: NodeId) -> Result<PreActivation> {
    let world = world_for_node(ctx, node)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(node) else {
        return Ok(PreActivation::None);
    };
    let type_attr = {
        let base = &parsed.document.base;
        let Some(tree) = base.get_node(node.node) else {
            return Ok(PreActivation::None);
        };
        let Some(element) = tree.data.downcast_element() else {
            return Ok(PreActivation::None);
        };
        // Only an `input` has the checkbox/radio activation behavior.
        if element.name.ns != html_namespace() || element.name.local.as_ref() != "input" {
            return Ok(PreActivation::None);
        }
        input_type(base, node.node)
    };
    Ok(match type_attr.as_str() {
        "checkbox" => {
            let checked = attr(&parsed.document.base, node.node, "checked").is_some();
            let previous = PreActivation::Checkbox { checked };
            set_presence_attr(&mut parsed, node, "checked", !checked);
            previous
        }
        "radio" => {
            let previous = radio_group_checked(&parsed.document.base, node.document, node.node);
            set_presence_attr(&mut parsed, node, "checked", true);
            PreActivation::Radio(previous)
        }
        _ => PreActivation::None,
    })
}

/// The legacy-canceled-activation behavior: undo the pre-activation change.
fn legacy_canceled_activation(ctx: &Ctx<'_>, node: NodeId, previous: &PreActivation) -> Result<()> {
    let world = world_for_node(ctx, node)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(node) else {
        return Ok(());
    };
    match previous {
        PreActivation::Checkbox { checked } => {
            set_presence_attr(&mut parsed, node, "checked", *checked);
        }
        PreActivation::Radio(Some(other)) => {
            set_presence_attr(&mut parsed, node, "checked", false);
            set_presence_attr(&mut parsed, *other, "checked", true);
        }
        PreActivation::Radio(None) => {
            set_presence_attr(&mut parsed, node, "checked", false);
        }
        PreActivation::None => {}
    }
    Ok(())
}

/// The post-click activation for a non-canceled click. A checkbox or radio
/// already changed checkedness in pre-activation, so only its events fire;
/// other controls run their activation behavior.
fn complete_activation(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    let world = world_for_node(ctx, node)?;
    let checkable = {
        let world = world.borrow();
        world.document(node).is_some_and(|parsed| {
            let base = &parsed.document.base;
            base.get_node(node.node)
                .and_then(|tree| tree.data.downcast_element())
                .is_some_and(|element| {
                    element.name.ns == html_namespace()
                        && element.name.local.as_ref() == "input"
                        && matches!(input_type(base, node.node).as_str(), "checkbox" | "radio")
                })
        })
    };
    if checkable {
        fire_checkable_events(ctx, node)
    } else {
        run_activation(ctx, node)
    }
}

/// Fires the `input` and `change` events a checkbox/radio activation produces.
fn fire_checkable_events(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    events::fire_trusted(ctx, EventTargetKey::Node(node), "input", true, false)?;
    events::fire_trusted(ctx, EventTargetKey::Node(node), "change", true, false)?;
    Ok(())
}

/// What a clicked control does.
enum Activation {
    None,
    Submit,
    Reset,
    ToggleCheckedness,
}

/// The checkbox/radio activation behavior: toggle (checkbox) or set (radio)
/// checkedness, unchecking the rest of the radio group, then fire `input` and
/// `change`
/// (<https://html.spec.whatwg.org/multipage/input.html#checkbox-state-(type=checkbox):activation-behavior>).
fn toggle_checkedness(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    let world = world_for_node(ctx, node)?;
    let changed = {
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(node) else {
            return Ok(());
        };
        let type_attr = {
            let base = &parsed.document.base;
            let Some(tree) = base.get_node(node.node) else {
                return Ok(());
            };
            let Some(element) = tree.data.downcast_element() else {
                return Ok(());
            };
            if element.name.ns != html_namespace() || element.name.local.as_ref() != "input" {
                return Ok(());
            }
            input_type(base, node.node)
        };
        match type_attr.as_str() {
            "checkbox" => {
                let checked = attr(&parsed.document.base, node.node, "checked").is_some();
                set_presence_attr(&mut parsed, node, "checked", !checked);
                true
            }
            "radio" => {
                // A checked radio cannot be unchecked by clicking.
                if attr(&parsed.document.base, node.node, "checked").is_some() {
                    false
                } else {
                    for member in radio_group(&parsed.document.base, node.document, node.node) {
                        set_presence_attr(&mut parsed, member, "checked", member == node);
                    }
                    true
                }
            }
            _ => false,
        }
    };
    if changed {
        events::fire_trusted(ctx, EventTargetKey::Node(node), "input", true, false)?;
        events::fire_trusted(ctx, EventTargetKey::Node(node), "change", true, false)?;
    }
    Ok(())
}

/// The activation behavior of a clicked control: a submit button submits its
/// form owner with itself as submitter, a reset button resets it, and a
/// checkbox or radio updates its checkedness
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-button-element:activation-behavior>).
fn run_activation(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    let world = world_for_node(ctx, node)?;
    let (activation, form) = {
        let world = world.borrow();
        let Some(parsed) = world.document(node) else {
            return Ok(());
        };
        let base = &parsed.document.base;
        let Some(tree) = base.get_node(node.node) else {
            return Ok(());
        };
        let Some(element) = tree.data.downcast_element() else {
            return Ok(());
        };
        if element.name.ns != html_namespace() {
            return Ok(());
        }
        let activation = match element.name.local.as_ref() {
            "input" => match input_type(base, node.node).as_str() {
                "submit" => Activation::Submit,
                "reset" => Activation::Reset,
                "checkbox" | "radio" => Activation::ToggleCheckedness,
                _ => Activation::None,
            },
            "button" => match input_type(base, node.node).as_str() {
                // The missing and invalid value defaults are both Auto (submit).
                // `input_type` of a button reads its own `type` attribute the
                // same way; missing or invalid falls through to submit.
                "reset" => Activation::Reset,
                _ => Activation::Submit,
            },
            _ => Activation::None,
        };
        let form = form_owner(base, node.node).map(|form| NodeId {
            document: node.document,
            node: form,
        });
        (activation, form)
    };
    match activation {
        Activation::None => Ok(()),
        Activation::ToggleCheckedness => toggle_checkedness(ctx, node),
        Activation::Submit | Activation::Reset => {
            let Some(form) = form else {
                return Ok(());
            };
            let form_value = wrap_node(ctx, form)?;
            let Some(form_object) = form_value.as_object() else {
                return Ok(());
            };
            let button_value = wrap_node(ctx, node)?;
            match activation {
                Activation::Submit => {
                    if let Ok(function) = form_object.get::<_, Function>("requestSubmit")
                        && function
                            .call::<_, ()>((This(form_object.clone()), button_value))
                            .is_err()
                    {
                        // Clear the exception the page's handler threw.
                        let _ = ctx.catch();
                    }
                }
                Activation::Reset => {
                    if let Ok(function) = form_object.get::<_, Function>("reset")
                        && function.call::<_, ()>((This(form_object.clone()),)).is_err()
                    {
                        let _ = ctx.catch();
                    }
                }
                Activation::None | Activation::ToggleCheckedness => {}
            }
            Ok(())
        }
    }
}

pub(crate) fn install_webdriver_bridge(ctx: &Ctx<'_>) -> Result<()> {
    let globals = crate::js::bridge::object(ctx)?;
    globals.set(
        "__tb_webdriver_click",
        rquickjs::prelude::Func::from(webdriver_click),
    )?;
    globals.set(
        "__tb_webdriver_element",
        rquickjs::prelude::Func::from(webdriver_element),
    )?;
    globals.set(
        "__tbActivate",
        rquickjs::prelude::Func::from(activate_element),
    )?;
    globals.set(
        "__tbSetNativeValue",
        rquickjs::prelude::Func::from(set_native_value),
    )?;
    Ok(())
}

/// Runs the activation behavior for an element the input paths clicked, so a
/// real click toggles a checkbox or submits a form
/// (<https://html.spec.whatwg.org/multipage/interaction.html#activation-behavior>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn activate_element<'js>(ctx: Ctx<'js>, element: Value<'js>) -> Result<()> {
    let Some(node) = host_node_id(&ctx, &element) else {
        return Ok(());
    };
    // A disabled control eats the click
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-fe-disabled>).
    {
        let world = world_for_node(&ctx, node)?;
        let world = world.borrow();
        if let Some(parsed) = world.document(node)
            && is_actually_disabled(&parsed.document.base, node.node)
        {
            return Ok(());
        }
    }
    run_activation(&ctx, node)
}

/// Sets an input or textarea's value without invoking an author-defined
/// `value` setter. Native user editing changes the control's internal value;
/// libraries that track the value (notably React's input value tracker)
/// observe the subsequent `input` event against the previously tracked value,
/// so routing the change through the IDL setter would make the edit invisible
/// to them
/// (<https://html.spec.whatwg.org/multipage/interaction.html#input-events>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn set_native_value<'a>(ctx: Ctx<'a>, element: Value<'a>, value: LegacyNullString) -> Result<()> {
    let Some(node) = host_node_id(&ctx, &element) else {
        return Err(Exception::throw_type(&ctx, "argument is not an element"));
    };
    let world = world_for_node(&ctx, node)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(node) else {
        return Err(Exception::throw_type(&ctx, "no document"));
    };
    if parsed
        .document
        .base
        .get_node(node.node)
        .is_none_or(|tree| tree.data.downcast_element().is_none())
    {
        return Err(Exception::throw_type(&ctx, "argument is not an element"));
    }
    // Same write the IDL setter performs, minus the author-visible entry
    // point (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
    // Blitz keeps no control value store (see `forms.rs`), so the `value`
    // content attribute carries it; the dirty-value flag is a known cutover
    // gap. Going through the tree mutator also keeps Blitz's text-input
    // state in sync.
    write_attr(&mut parsed, node, "value", Some(&value.0));
    Ok(())
}

/// The `WebDriver` "element click" step: a trusted click at the element, with
/// focus moved to it first when it is focusable.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn webdriver_click<'js>(ctx: Ctx<'js>, element: Value<'js>) -> Result<()> {
    let Some(node) = host_node_id(&ctx, &element) else {
        return Err(Exception::throw_type(&ctx, "not an element"));
    };
    // A disabled control eats the click: no focus move, no event
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-fe-disabled>).
    {
        let world = world_for_node(&ctx, node)?;
        let world = world.borrow();
        if let Some(parsed) = world.document(node)
            && is_actually_disabled(&parsed.document.base, node.node)
        {
            return Ok(());
        }
    }
    if is_focusable(&ctx, node)? {
        focus_node(&ctx, node)?;
    }
    if events::fire_trusted_click(&ctx, EventTargetKey::Node(node))? {
        run_activation(&ctx, node)?;
    }
    Ok(())
}
