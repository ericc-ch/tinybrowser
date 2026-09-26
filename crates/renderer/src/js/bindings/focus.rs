//! Focus, activation behavior, and the `WebDriver` bridge.

use super::{events, host_node_id, webdriver_element, world_for_node, wrap_node};

use std::rc::Rc;

use dom::{NodeId, NodeKind, html_namespace};

use rquickjs::{Class, Ctx, Exception, Function, Object, Result, Value, prelude::This};

use crate::js::events::EventTargetRef;

use crate::js::world::EventTargetKey;

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
/// of the decision; the element-name, disabled, and connection rules are
/// (<https://html.spec.whatwg.org/multipage/interaction.html#focusable-area>).
pub(crate) fn is_focusable(ctx: &Ctx<'_>, node: NodeId) -> Result<bool> {
    let world = world_for_node(ctx, node)?;
    let world = world.borrow();
    let Some(parsed) = world.document(node) else {
        return Ok(false);
    };
    let Some(kind) = parsed.dom.kind(node) else {
        return Ok(false);
    };
    let NodeKind::Element { name, .. } = kind else {
        return Ok(false);
    };
    if !parsed.dom.is_connected(node) || is_actually_disabled(&parsed.dom, node) {
        return Ok(false);
    }
    // `tabindex` and `contenteditable` apply to SVG elements too.
    if parsed.dom.attribute(node, "tabindex").is_some() || is_editable(&parsed.dom, node) {
        return Ok(true);
    }
    if name.ns != html_namespace() {
        return Ok(false);
    }
    Ok(match name.local.as_ref() {
        "input" => !is_hidden_input(&parsed.dom, node),
        "a" | "area" => parsed.dom.attribute(node, "href").is_some(),
        "button" | "iframe" | "select" | "textarea" => true,
        _ => false,
    })
}

/// The HTML "actually disabled" check for the form controls the engine
/// supports, including descendants of a disabled `fieldset` that are not
/// inside its first `legend`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-fe-disabled>).
fn is_actually_disabled(dom: &dom::Dom, node: NodeId) -> bool {
    if dom.attribute(node, "disabled").is_some()
        && matches!(
            node_local_name(dom, node).as_deref(),
            Some("button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset")
        )
    {
        return true;
    }
    let mut cursor = dom.parent(node);
    while let Some(parent) = cursor {
        if node_local_name(dom, parent).as_deref() == Some("fieldset")
            && dom.attribute(parent, "disabled").is_some()
        {
            let first_legend = dom
                .children(parent)
                .into_iter()
                .flatten()
                .find(|&child| node_local_name(dom, child).as_deref() == Some("legend"));
            if let Some(legend) = first_legend {
                let mut inner = Some(node);
                while let Some(current) = inner {
                    if current == legend {
                        return false;
                    }
                    inner = dom.parent(current);
                }
            }
            return true;
        }
        cursor = dom.parent(parent);
    }
    false
}

/// Whether the element is an editing host through `contenteditable`,
/// inheriting the value from ancestors. Only `""`, `true`, and
/// `plaintext-only` enable editing
/// (<https://html.spec.whatwg.org/multipage/interaction.html#attr-contenteditable>).
fn is_editable(dom: &dom::Dom, node: NodeId) -> bool {
    let mut cursor = Some(node);
    while let Some(current) = cursor {
        if let Some(value) = dom.attribute(current, "contenteditable") {
            if value.is_empty()
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("plaintext-only")
            {
                return true;
            }
            if value.eq_ignore_ascii_case("false") {
                return false;
            }
        }
        cursor = dom.parent(current);
    }
    false
}

fn is_hidden_input(dom: &dom::Dom, node: NodeId) -> bool {
    dom.attribute(node, "type")
        .is_some_and(|kind| kind.eq_ignore_ascii_case("hidden"))
}

fn node_local_name(dom: &dom::Dom, node: NodeId) -> Option<String> {
    match dom.kind(node) {
        Some(NodeKind::Element { name, .. }) if name.ns == html_namespace() => {
            Some(name.local.to_string())
        }
        _ => None,
    }
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
    let target = |node| EventTargetRef {
        key: EventTargetKey::Node(node),
        world: Rc::clone(&world),
    };
    world.borrow_mut().set_active_element(document, None);
    if let Some(previous) = previous {
        events::fire_trusted_with_related(
            ctx,
            EventTargetKey::Node(previous),
            "blur",
            false,
            false,
            Some(target(node)),
        )?;
        // A handler may have moved focus; the spec's focus update steps stop
        // when the focused area changed during the blur chain, and `focusout`
        // carries the new area as its related target.
        if let Some(moved) = world.borrow().active_element(document) {
            events::fire_trusted_with_related(
                ctx,
                EventTargetKey::Node(previous),
                "focusout",
                true,
                false,
                Some(target(moved)),
            )?;
            return Ok(());
        }
        events::fire_trusted_with_related(
            ctx,
            EventTargetKey::Node(previous),
            "focusout",
            true,
            false,
            Some(target(node)),
        )?;
    }
    // Handlers may have made the target unfocusable; browsers then do not
    // designate or fire on it.
    if !is_focusable(ctx, node)? {
        return Ok(());
    }
    world.borrow_mut().set_active_element(document, Some(node));
    let related = previous.map(target);
    events::fire_trusted_with_related(
        ctx,
        EventTargetKey::Node(node),
        "focus",
        false,
        false,
        related.clone(),
    )?;
    events::fire_trusted_with_related(
        ctx,
        EventTargetKey::Node(node),
        "focusin",
        true,
        false,
        related,
    )?;
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
        if is_actually_disabled(&parsed.dom, node) || world.click_in_progress(node) {
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
    Checkbox {
        checked: bool,
        indeterminate: bool,
    },
    Radio(Option<NodeId>),
}

/// The legacy-pre-activation behavior: a checkbox toggles, a radio becomes
/// checked and remembers the group's previous checked radio.
fn legacy_pre_activation(ctx: &Ctx<'_>, node: NodeId) -> Result<PreActivation> {
    let world = world_for_node(ctx, node)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(node) else {
        return Ok(PreActivation::None);
    };
    let dom = &mut parsed.dom;
    let Some(NodeKind::Element { name, .. }) = dom.kind(node) else {
        return Ok(PreActivation::None);
    };
    // Only an `input` has the checkbox/radio activation behavior.
    if name.ns != html_namespace() || name.local.as_ref() != "input" {
        return Ok(PreActivation::None);
    }
    let type_attr = dom
        .attribute(node, "type")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    Ok(match type_attr.as_str() {
        "checkbox" => {
            let previous = PreActivation::Checkbox {
                checked: dom.checkedness(node),
                indeterminate: dom.indeterminate(node),
            };
            let next = !dom.checkedness(node);
            dom.set_input_checkedness(node, next);
            dom.set_indeterminate(node, false);
            previous
        }
        "radio" => {
            let previous = dom.radio_group_checked(node);
            dom.set_input_checkedness(node, true);
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
    let dom = &mut parsed.dom;
    match previous {
        PreActivation::Checkbox {
            checked,
            indeterminate,
        } => {
            dom.set_input_checkedness(node, *checked);
            dom.set_indeterminate(node, *indeterminate);
        }
        PreActivation::Radio(Some(other)) => {
            dom.set_input_checkedness(node, false);
            dom.set_input_checkedness(*other, true);
        }
        PreActivation::Radio(None) => {
            dom.set_input_checkedness(node, false);
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
            let dom = &parsed.dom;
            matches!(
                dom.kind(node),
                Some(NodeKind::Element { name, .. })
                    if name.ns == html_namespace() && name.local.as_ref() == "input"
            ) && dom.attribute(node, "type").is_some_and(|value| {
                let value = value.trim().to_ascii_lowercase();
                value == "checkbox" || value == "radio"
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
        let world = world.borrow_mut();
        let Some(mut parsed) = world.document_mut(node) else {
            return Ok(());
        };
        let dom = &mut parsed.dom;
        let Some(NodeKind::Element { name, .. }) = dom.kind(node) else {
            return Ok(());
        };
        if name.ns != html_namespace() || name.local.as_ref() != "input" {
            return Ok(());
        }
        let type_attr = dom
            .attribute(node, "type")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match type_attr.as_str() {
            "checkbox" => {
                let next = !dom.checkedness(node);
                dom.set_input_checkedness(node, next);
                true
            }
            "radio" => {
                // A checked radio cannot be unchecked by clicking.
                if dom.checkedness(node) {
                    false
                } else {
                    dom.set_input_checkedness(node, true);
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
        let dom = &parsed.dom;
        let Some(NodeKind::Element { name, .. }) = dom.kind(node) else {
            return Ok(());
        };
        if name.ns != html_namespace() {
            return Ok(());
        }
        let type_attr = |dom: &dom::Dom| {
            dom.attribute(node, "type")
                .map(|value| value.trim().to_ascii_lowercase())
        };
        let activation = match name.local.as_ref() {
            "input" => match type_attr(dom).as_deref() {
                Some("submit") => Activation::Submit,
                Some("reset") => Activation::Reset,
                Some("checkbox" | "radio") => Activation::ToggleCheckedness,
                _ => Activation::None,
            },
            "button" => match type_attr(dom).as_deref() {
                // The missing and invalid value defaults are both Auto (submit).
                Some("reset") => Activation::Reset,
                _ => Activation::Submit,
            },
            _ => Activation::None,
        };
        (activation, dom.form_owner(node))
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

/// The `WebDriver` element bridge. Page script can still call it by name and
/// forge `isTrusted` events; it is not enumerable, so `Window` enumeration
/// and idlharness do not see it.
pub(crate) fn install_webdriver_bridge(ctx: &Ctx<'_>, globals: &Object<'_>) -> Result<()> {
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
    ctx.eval::<(), _>(
        "['__tb_webdriver_click','__tb_webdriver_element','__tbActivate']\
         .forEach(function(k){Object.defineProperty(globalThis,k,{writable:false,configurable:false,enumerable:false});});",
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
            && is_actually_disabled(&parsed.dom, node)
        {
            return Ok(());
        }
    }
    run_activation(&ctx, node)
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
            && is_actually_disabled(&parsed.dom, node)
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
