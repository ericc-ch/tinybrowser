//! Focus, activation behavior, and the `WebDriver` bridge.

use super::node::element;
use super::{LegacyNullString, events, host_node_id, webdriver_element, world_for_node, wrap_node};

use markup5ever::{LocalName, Namespace, QualName};
use rquickjs::{Ctx, Exception, Function, Result, Value, prelude::This};

use crate::documents::JournalEntry;
use crate::dom::tree::{BlitzId, NodeId, attr, form_owner_of, html_namespace, is_connected};
use crate::js::world::EventTargetKey;
use crate::js::{FrameNavigation, NavigationTarget, World};

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
    Ok(match element.name.local.as_str() {
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
            .find(|attribute| attribute.name.local.as_str() == local)
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

/// The `type` of an `input`, lowercased and trimmed; missing means `text`.
fn input_type(base: &blitz_dom::BaseDocument, node: BlitzId) -> String {
    attr(base, node, "type")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// The group's currently checked radio, if any. An empty or missing `name`
/// forms no group, so a nameless radio is only its own group member
/// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
fn radio_group_checked(
    base: &blitz_dom::BaseDocument,
    document: u32,
    node: BlitzId,
) -> Option<NodeId> {
    if checkedness(base, node) {
        return Some(NodeId { document, node });
    }
    let name = attr(base, node, "name").unwrap_or_default();
    crate::dom::forms::radio_group_members(base, document, name, node)
        .into_iter()
        .find(|member| checkedness(base, member.node))
}

/// An input's checkedness: the dirty slot when set, otherwise the `checked`
/// attribute (<https://html.spec.whatwg.org/multipage/input.html#concept-input-checked-dirty>).
fn checkedness(base: &blitz_dom::BaseDocument, node: BlitzId) -> bool {
    base.get_node(node)
        .and_then(|target| target.data.downcast_element())
        .is_some_and(blitz_dom::ElementData::checkedness)
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
            events::fire_trusted(ctx, EventTargetKey::Node(previous), "focusout", true, false)?;
            return Ok(());
        }
        events::fire_trusted(ctx, EventTargetKey::Node(previous), "focusout", true, false)?;
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
        if is_actually_disabled(&parsed.document.base, node.node) || world.click_in_progress(node) {
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
        let event = super::host::instance(ctx, events::JsEvent::uninitialized())?;
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
        if element.name.ns != html_namespace() || element.name.local.as_str() != "input" {
            return Ok(PreActivation::None);
        }
        input_type(base, node.node)
    };
    Ok(match type_attr.as_str() {
        "checkbox" => {
            let checked = checkedness(&parsed.document.base, node.node);
            let previous = PreActivation::Checkbox { checked };
            let mut mutator = parsed.document.base.mutate();
            mutator.set_checkedness(node.node, !checked);
            // Activation clears indeterminateness
            // (<https://html.spec.whatwg.org/multipage/input.html#checkbox-state-(type=checkbox):activation-behavior>).
            mutator.set_indeterminate(node.node, false);
            previous
        }
        "radio" => {
            let previous = radio_group_checked(&parsed.document.base, node.document, node.node);
            let name = attr(&parsed.document.base, node.node, "name").unwrap_or_default();
            let members = crate::dom::forms::radio_group_members(
                &parsed.document.base,
                node.document,
                name,
                node.node,
            );
            let mut mutator = parsed.document.base.mutate();
            for member in members {
                mutator.set_checkedness(member.node, false);
            }
            mutator.set_checkedness(node.node, true);
            mutator.set_indeterminate(node.node, false);
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
            let mut mutator = parsed.document.base.mutate();
            mutator.set_checkedness(node.node, *checked);
            // A canceled activation restores the pre-click state, including
            // indeterminateness; a handler that set it during the click must
            // not leak that change
            // (<https://html.spec.whatwg.org/multipage/input.html#canceled-activation-steps>).
            mutator.set_indeterminate(node.node, false);
        }
        PreActivation::Radio(Some(other)) => {
            let mut mutator = parsed.document.base.mutate();
            mutator.set_checkedness(node.node, false);
            mutator.set_checkedness(other.node, true);
        }
        PreActivation::Radio(None) => {
            parsed
                .document
                .base
                .mutate()
                .set_checkedness(node.node, false);
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
                        && element.name.local.as_str() == "input"
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
    events::fire_trusted_composed(ctx, EventTargetKey::Node(node), "input", true, false)?;
    events::fire_trusted(ctx, EventTargetKey::Node(node), "change", true, false)?;
    Ok(())
}

/// What a clicked control does.
enum Activation {
    None,
    Submit,
    Reset,
    ToggleCheckedness,
    /// Follow an `a`/`area` hyperlink in place
    /// (<https://html.spec.whatwg.org/multipage/text-level-semantics.html#the-a-element:activation-behaviour>).
    Link,
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
            if element.name.ns != html_namespace() || element.name.local.as_str() != "input" {
                return Ok(());
            }
            input_type(base, node.node)
        };
        match type_attr.as_str() {
            "checkbox" => {
                let checked = checkedness(&parsed.document.base, node.node);
                let mut mutator = parsed.document.base.mutate();
                mutator.set_checkedness(node.node, !checked);
                mutator.set_indeterminate(node.node, false);
                true
            }
            "radio" => {
                // A checked radio cannot be unchecked by clicking.
                if checkedness(&parsed.document.base, node.node) {
                    false
                } else {
                    // The clicked radio joins its group and unchecks the
                    // rest
                    // (<https://html.spec.whatwg.org/multipage/input.html#radio-button-state>).
                    let name = attr(&parsed.document.base, node.node, "name")
                        .unwrap_or_default()
                        .to_owned();
                    let members = crate::dom::forms::radio_group_members(
                        &parsed.document.base,
                        node.document,
                        &name,
                        node.node,
                    );
                    let mut mutator = parsed.document.base.mutate();
                    for member in members {
                        mutator.set_checkedness(member.node, false);
                    }
                    mutator.set_checkedness(node.node, true);
                    mutator.set_indeterminate(node.node, false);
                    true
                }
            }
            _ => false,
        }
    };
    if changed {
        events::fire_trusted_composed(ctx, EventTargetKey::Node(node), "input", true, false)?;
        events::fire_trusted(ctx, EventTargetKey::Node(node), "change", true, false)?;
    }
    Ok(())
}

/// Whether an element cannot navigate
/// (<https://html.spec.whatwg.org/multipage/links.html#cannot-navigate>): its
/// node document is not fully active, or — for anything but an `a` element,
/// which keeps the web-compatible exception — it is not connected. Following a
/// hyperlink and form submission both start by returning for such an element.
///
/// "Fully active" is "the node document is this realm's active document": a
/// document built by `createDocument()` has no browsing context at all, and a
/// document its frame has navigated away from is no longer active.
pub(super) fn cannot_navigate(world: &World, node: NodeId) -> bool {
    if !world.is_main_document(node) {
        return true;
    }
    let Some(parsed) = world.document(node) else {
        return true;
    };
    let base = &parsed.document.base;
    let anchor = node_local_name(base, node.node).as_deref() == Some("a");
    !anchor && !is_connected(base, node.node)
}

/// The referrer policy a followed hyperlink navigates with
/// (<https://html.spec.whatwg.org/multipage/links.html#hyperlink-referrer-policy>):
/// the `noreferrer` link type wins, otherwise the `referrerpolicy` content
/// attribute, whose keywords are the referrer policies and whose invalid value
/// default is the empty string. An empty policy leaves the initiator document's
/// policy in force.
fn hyperlink_referrer_policy(base: &blitz_dom::BaseDocument, node: BlitzId) -> String {
    if attr(base, node, "rel")
        .unwrap_or_default()
        .split_ascii_whitespace()
        .any(|token| token.eq_ignore_ascii_case("noreferrer"))
    {
        return "no-referrer".to_owned();
    }
    element::reflect_enum(
        attr(base, node, "referrerpolicy"),
        element::REFERRER_POLICY_KEYWORDS,
        "",
        "",
    )
}

/// The label element's activation behavior: a click on the label or a
/// non-interactive descendant runs synthetic click activation steps on the
/// labeled control
/// (<https://html.spec.whatwg.org/multipage/forms.html#the-label-element:activation-behavior>).
pub(crate) fn run_label_activation(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    let label = {
        let world = world_for_node(ctx, node)?;
        let world = world.borrow();
        let Some(parsed) = world.document(node) else {
            return Ok(());
        };
        let base = &parsed.document.base;
        let mut cursor = Some(node.node);
        let mut label = None;
        while let Some(current) = cursor {
            if crate::dom::tree::is_html_element(base, current, "label") {
                label = Some(NodeId {
                    document: node.document,
                    node: current,
                });
                break;
            }
            cursor = base.get_node(current).and_then(|target| target.parent);
        }
        let Some(label) = label else {
            return Ok(());
        };
        // A click on interactive content, or on a descendant of it, does not
        // forward to the labeled control. The label itself is the target when
        // the label was clicked, so it does not count here.
        let mut cursor = Some(node.node);
        while let Some(current) = cursor {
            if current == label.node {
                break;
            }
            if is_interactive_content(base, current) {
                return Ok(());
            }
            cursor = base.get_node(current).and_then(|target| target.parent);
        }
        label
    };
    // The label's `control` getter owns the `for`/descendant lookup and
    // labelable-element rules
    // (<https://html.spec.whatwg.org/multipage/forms.html#dom-label-control>).
    let label_value = wrap_node(ctx, label)?;
    let Some(label_object) = label_value.as_object() else {
        return Ok(());
    };
    let control: Value<'_> = label_object.get("control")?;
    if let Some(control) = host_node_id(ctx, &control) {
        // A click on the labeled control itself, or inside it, already ran
        // that control's own activation behavior. Forwarding it again would
        // recurse (a labelable but non-interactive control such as `meter`
        // does not stop the walk above).
        let inside_control = {
            let world = world_for_node(ctx, node)?;
            let world = world.borrow();
            let Some(parsed) = world.document(node) else {
                return Ok(());
            };
            let base = &parsed.document.base;
            let mut cursor = Some(node.node);
            let mut inside = false;
            while let Some(current) = cursor {
                if current == control.node {
                    inside = true;
                    break;
                }
                if current == label.node {
                    break;
                }
                cursor = base.get_node(current).and_then(|target| target.parent);
            }
            inside
        };
        if !inside_control {
            element_click(ctx, control)?;
        }
    }
    Ok(())
}

/// Whether `node` is interactive content for label forwarding
/// (<https://html.spec.whatwg.org/multipage/dom.html#interactive-content>).
fn is_interactive_content(base: &blitz_dom::BaseDocument, node: BlitzId) -> bool {
    let Some(target) = base.get_node(node) else {
        return false;
    };
    let Some(element) = target.data.downcast_element() else {
        return false;
    };
    if element.name.ns != html_namespace() {
        return false;
    }
    match element.name.local.as_str() {
        "a" => attr(base, node, "href").is_some(),
        "audio" | "video" => attr(base, node, "controls").is_some(),
        "button" | "details" | "iframe" | "embed" | "select" | "textarea" => true,
        "input" => {
            crate::dom::input::input_type(base, node) != Some(crate::dom::input::InputType::Hidden)
        }
        "img" => attr(base, node, "usemap").is_some(),
        _ => false,
    }
}

/// The activation behavior of a clicked control: a submit button submits its
/// form owner with itself as submitter, a reset button resets it, and a
/// checkbox or radio updates its checkedness
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-button-element:activation-behavior>).
fn run_activation(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    let world = world_for_node(ctx, node)?;
    let (activation, form, consumes_activation) = {
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
        let mut consumes_activation = false;
        let activation = match element.name.local.as_str() {
            "input" => {
                let kind = input_type(base, node.node);
                // The input activation behavior shows the picker for these
                // states, which consumes the transient activation
                // (<https://html.spec.whatwg.org/multipage/input.html#input-activation-behavior>).
                consumes_activation = matches!(
                    kind.as_str(),
                    "color" | "date" | "datetime-local" | "file" | "month" | "time" | "week"
                );
                match kind.as_str() {
                    "submit" => Activation::Submit,
                    "reset" => Activation::Reset,
                    "checkbox" | "radio" => Activation::ToggleCheckedness,
                    _ => Activation::None,
                }
            }
            "button" => {
                // A button with a command but no explicit `type` runs the
                // command behavior instead of submitting
                // (<https://html.spec.whatwg.org/multipage/form-elements.html#the-button-element:activation-behavior>).
                let explicit = crate::dom::tree::attr(base, node.node, "type");
                let has_command = crate::dom::tree::attr(base, node.node, "command").is_some()
                    || crate::dom::tree::attr(base, node.node, "commandfor").is_some();
                if explicit.is_none() && has_command {
                    Activation::None
                } else {
                    match input_type(base, node.node).as_str() {
                        // The missing and invalid value defaults are both Auto
                        // (submit); `input_type` of a button reads its own
                        // `type` attribute the same way.
                        "reset" => Activation::Reset,
                        _ => Activation::Submit,
                    }
                }
            }
            // An anchor without `href` has no activation behavior.
            "a" | "area" if crate::dom::tree::attr(base, node.node, "href").is_some() => {
                Activation::Link
            }
            _ => Activation::None,
        };
        let form = form_owner_of(base, node.document, node.node);
        (activation, form, consumes_activation)
    };
    if consumes_activation {
        world.borrow_mut().consume_user_activation();
    }
    match activation {
        Activation::None => Ok(()),
        Activation::ToggleCheckedness => toggle_checkedness(ctx, node),
        Activation::Link => follow_link(ctx, node),
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
                        && function
                            .call::<_, ()>((This(form_object.clone()),))
                            .is_err()
                    {
                        let _ = ctx.catch();
                    }
                }
                Activation::None | Activation::ToggleCheckedness | Activation::Link => {}
            }
            Ok(())
        }
    }
}

/// Follows an `a`/`area` hyperlink after its activation behavior
/// (<https://html.spec.whatwg.org/multipage/links.html#following-hyperlinks-2>).
fn follow_link(ctx: &Ctx<'_>, node: NodeId) -> Result<()> {
    let world = world_for_node(ctx, node)?;
    if cannot_navigate(&world.borrow(), node) {
        return Ok(());
    }
    let (spec, target, referrer_policy) = {
        let world = world.borrow();
        let Some(parsed) = world.document(node) else {
            return Ok(());
        };
        let base = &parsed.document.base;
        (
            crate::dom::tree::attr(base, node.node, "href").map(str::to_owned),
            crate::dom::tree::attr(base, node.node, "target").map(str::to_owned),
            hyperlink_referrer_policy(base, node.node),
        )
    };
    let Some(spec) = spec else {
        return Ok(());
    };
    // The target picks the navigable at navigation time, so a frame inserted
    // between the click and the drain is still eligible.
    let navigation =
        FrameNavigation::get(NavigationTarget::Target(target.unwrap_or_default()), spec)
            .with_referrer_policy(referrer_policy);
    world.borrow_mut().queue_frame_navigation(navigation);
    Ok(())
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
    globals.set(
        "__tbUserActivationActive",
        rquickjs::prelude::Func::from(user_activation_active),
    )?;
    globals.set(
        "__tbUserActivationSticky",
        rquickjs::prelude::Func::from(user_activation_sticky),
    )?;
    globals.set(
        "__tbConsumeUserActivation",
        rquickjs::prelude::Func::from(consume_user_activation),
    )?;
    Ok(())
}

/// Whether the current realm has unexpired transient activation
/// (<https://html.spec.whatwg.org/multipage/interaction.html#dom-useractivation-isactive>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn user_activation_active(ctx: Ctx<'_>) -> Result<bool> {
    Ok(super::world(&ctx)?.borrow().user_activation_active())
}

/// Whether the current realm was ever activated
/// (<https://html.spec.whatwg.org/multipage/interaction.html#dom-useractivation-hasbeenactive>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn user_activation_sticky(ctx: Ctx<'_>) -> Result<bool> {
    Ok(super::world(&ctx)?.borrow().user_activation_sticky())
}

/// Consumes the current realm's transient activation
/// (<https://html.spec.whatwg.org/multipage/interaction.html#consume-user-activation>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn consume_user_activation(ctx: Ctx<'_>) -> Result<bool> {
    Ok(super::world(&ctx)?.borrow_mut().consume_user_activation())
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
    let local = parsed
        .document
        .base
        .get_node(node.node)
        .and_then(|tree| tree.data.downcast_element())
        .filter(|element| element.name.ns == crate::dom::tree::html_namespace())
        .map(|element| element.name.local.to_string());
    match local.as_deref() {
        Some("input") => {
            let Some(kind) = crate::dom::input::input_type(&parsed.document.base, node.node) else {
                return Ok(());
            };
            if kind.value_mode() == crate::dom::input::ValueMode::Current {
                let sanitized =
                    crate::dom::input::sanitize(&parsed.document.base, node.node, kind, &value.0);
                parsed
                    .document
                    .base
                    .mutate()
                    .set_dirty_value(node.node, sanitized);
                parsed.document.clear_pristine_value(node.node);
            } else {
                write_attr(&mut parsed, node, "value", Some(&value.0));
            }
        }
        Some("textarea") => {
            parsed
                .document
                .base
                .mutate()
                .set_dirty_value(node.node, value.0);
            parsed.document.clear_pristine_value(node.node);
        }
        _ => write_attr(&mut parsed, node, "value", Some(&value.0)),
    }
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
    // A WebDriver click is a trusted user interaction, so it grants
    // activation before the click event reaches the page
    // (<https://html.spec.whatwg.org/multipage/interaction.html#activation-notification>).
    world_for_node(&ctx, node)?.borrow_mut().activate_user();
    if is_focusable(&ctx, node)? {
        focus_node(&ctx, node)?;
    }
    if events::fire_trusted_click(&ctx, EventTargetKey::Node(node))? {
        run_activation(&ctx, node)?;
    }
    Ok(())
}
