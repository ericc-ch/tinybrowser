//! Focus, activation behavior, and the `WebDriver` bridge.

use super::{events, host_node_id, webdriver_element, world_for_node};

use std::rc::Rc;

use dom::{NodeId, NodeKind, html_namespace};

use rquickjs::{Class, Ctx, Exception, Object, Result, Value};

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
        let event = Class::instance(ctx.clone(), events::JsEvent::uninitialized())?;
        event.borrow().initialize("click".to_owned(), true, true);
        events::dispatch_event(ctx, EventTargetKey::Node(node), &event)?;
        Ok(())
    })();
    world.borrow_mut().set_click_in_progress(node, false);
    result
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
    ctx.eval::<(), _>(
        "['__tb_webdriver_click','__tb_webdriver_element']\
         .forEach(function(k){Object.defineProperty(globalThis,k,{writable:false,configurable:false,enumerable:false});});",
    )?;
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
            && is_actually_disabled(&parsed.dom, node)
        {
            return Ok(());
        }
    }
    if is_focusable(&ctx, node)? {
        focus_node(&ctx, node)?;
    }
    events::fire_trusted(&ctx, EventTargetKey::Node(node), "click", true, true)?;
    Ok(())
}
