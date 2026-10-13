//! The single `Node` wrapper class and its members.

use super::{
    CollectionKind, LegacyNullString, Trace, adopt_into_document, deref_weak,
    fixup_focus_after_removal, host, import_attr, live_collection, make_weak,
    schedule_mutation_delivery, throw_dom, world, world_for_node, wrap_node,
};

use blitz_dom::NodeData;

use crate::dom_string::DomString;

use rquickjs::{Ctx, Exception, Persistent, Result, Value};

use crate::dom::insertion::{
    iframe_containers_in_subtrees, place_journaled, replace_all_journaled, unlink_journaled,
};
use crate::dom::tree::{NodeId, html_namespace, sibling};

use crate::documents::JournalEntry;
use crate::js::world::{Handle, NodeReference, Wrapper};

pub(super) mod core;
pub(super) mod document;
pub(super) mod element;
pub(super) mod forms;
pub(super) mod html_elements;
pub(super) mod media;

pub(super) use core::{compare_node_position, insert_converted_node};
pub(super) use document::{
    construct_node, construct_node_in_frame, create_node, install_custom_construction,
};

pub(super) fn install(ctx: &Ctx<'_>) -> Result<()> {
    core::install(ctx)?;
    document::install(ctx)?;
    element::install(ctx)?;
    forms::install(ctx)?;
    media::install(ctx)?;
    html_elements::install(ctx)?;
    Ok(())
}

/// The single `Node` wrapper class: one native identity shared by every
/// node interface. Interface prototypes installed by the family modules
/// below copy its members; the constructor stays illegal.
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "Node")]
pub(crate) struct JsNode {
    pub(crate) handle: Handle,
}

impl super::host::SharedClass for JsNode {
    // https://webidl.spec.whatwg.org/#es-attributes
    // https://webidl.spec.whatwg.org/#es-operations
    fn require_interface(&self, ctx: &Ctx<'_>, interface: &str) -> Result<()> {
        super::host::require_node_interface(ctx, self.handle.0, interface)
    }
}

impl JsNode {
    pub(crate) fn node_id(&self) -> NodeId {
        self.handle.0
    }
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; create* is a document method"
)]
impl JsNode {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    // https://html.spec.whatwg.org/multipage/dom.html#dom-document-scripts
    #[qjs(get, rename = "scripts")]
    fn scripts<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let world_rc = world(&ctx)?;
        if let Some(saved) = world_rc.borrow().wrapper(self.handle.0, Wrapper::Scripts)
            && let Some(value) = deref_weak(&ctx, saved)?
        {
            return Ok(value);
        }
        let value = live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::HtmlScripts,
            Some("HTMLCollection"),
        )?;
        let weak = make_weak(&ctx, value.clone())?;
        world_rc.borrow_mut().intern_wrapper(
            self.handle.0,
            Wrapper::Scripts,
            Persistent::save(&ctx, weak),
        );
        Ok(value)
    }

    /// <https://dom.spec.whatwg.org/#dom-document-adoptnode>
    #[qjs(rename = "adoptNode")]
    fn document_adopt_node<'js>(&self, ctx: Ctx<'js>, node: Value<'js>) -> Result<Value<'js>> {
        match host::node_argument(&ctx, &node)? {
            NodeReference::Tree(id) => {
                let adopted = adopt_into_document(&ctx, self.handle.0, id)?;
                schedule_mutation_delivery(&ctx)?;
                wrap_node(&ctx, adopted)
            }
            NodeReference::Attribute { scope, id } => import_attr(&ctx, self.handle.0, scope, id),
        }
    }

    /// <https://html.spec.whatwg.org/multipage/dom.html#the-innertext-idl-attribute>
    #[qjs(get, rename = "innerText")]
    fn inner_text<'js>(&self, ctx: Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let text = super::inner_text::rendered_text(&ctx, self.handle.0)?;
        rquickjs::String::from_str(ctx, &text)
    }

    /// <https://html.spec.whatwg.org/multipage/dom.html#the-innertext-idl-attribute>
    #[qjs(get, rename = "outerText")]
    fn outer_text<'js>(&self, ctx: Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let text = super::inner_text::rendered_text(&ctx, self.handle.0)?;
        rquickjs::String::from_str(ctx, &text)
    }

    /// Setting `outerText` replaces the element with the rendered text
    /// fragment, then merges with adjacent text siblings (but does not
    /// fully normalize). Without a parent it throws; the setter is a no-op
    /// on non-HTML elements, which never install it.
    /// (<https://html.spec.whatwg.org/multipage/dom.html#the-innertext-idl-attribute>).
    #[expect(
        clippy::too_many_lines,
        reason = "outerText setter steps in spec order; splitting them is a refactor, not a lint fix"
    )]
    #[qjs(set, rename = "outerText")]
    fn set_outer_text(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let parent = {
            let world = world(&ctx)?;
            let world = world.borrow();
            world.document(self.handle.0).and_then(|parsed| {
                parsed
                    .document
                    .base
                    .get_node(self.handle.0.node)
                    .and_then(|node| node.parent)
                    .map(|parent| NodeId {
                        document: self.handle.0.document,
                        node: parent,
                    })
            })
        };
        let Some(parent) = parent else {
            return Err(throw_dom(
                &ctx,
                "NoModificationAllowedError",
                "outerText needs a parent",
            ));
        };
        // Replacing the document element would put text under `Document`.
        let is_document_parent = {
            let world = world(&ctx)?;
            let world = world.borrow();
            world.document(self.handle.0).is_some_and(|parsed| {
                parsed
                    .document
                    .base
                    .get_node(parent.node)
                    .is_some_and(|node| matches!(node.data, NodeData::Document(_)))
            })
        };
        if is_document_parent {
            return Err(throw_dom(
                &ctx,
                "HierarchyRequestError",
                "outerText cannot replace the document element",
            ));
        }
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        // Child navigables die with the replaced element: collect their
        // containers before removal, destroy after the borrows drop.
        let iframes = iframe_containers_in_subtrees(&parsed, &[self.handle.0]);
        let mut added = crate::dom::text::rendered_text_fragment(&mut parsed, &value.0, true);
        for child in &added {
            place_journaled(&mut parsed, parent, *child, Some(self.handle.0));
        }
        unlink_journaled(&mut parsed, self.handle.0);
        // Merge with the previous text sibling, then the following one.
        if let Some(first) = added.first().copied() {
            let previous = sibling(&parsed.document.base, first.node, false).map(|node| NodeId {
                document: first.document,
                node,
            });
            if let Some(previous) = previous
                && crate::dom::text::is_mergeable_text_node(&parsed.document.base, previous.node)
                && crate::dom::text::is_mergeable_text_node(&parsed.document.base, first.node)
            {
                let previous_text = String::from(parsed.document.character_data(previous.node));
                let old_value = DomString::from(previous_text.clone());
                let mut data = previous_text;
                data.push_str(&String::from(parsed.document.character_data(first.node)));
                parsed
                    .document
                    .base
                    .mutate()
                    .set_node_text(previous.node, &data);
                parsed.document.record(JournalEntry::CharacterData {
                    target: previous,
                    old_value,
                });
                unlink_journaled(&mut parsed, first);
                if added.last() == Some(&first) {
                    added[0] = previous;
                } else {
                    added.remove(0);
                }
            }
        }
        if let Some(last) = added.last().copied() {
            let next = sibling(&parsed.document.base, last.node, true).map(|node| NodeId {
                document: last.document,
                node,
            });
            if let Some(next) = next
                && crate::dom::text::is_mergeable_text_node(&parsed.document.base, next.node)
                && crate::dom::text::is_mergeable_text_node(&parsed.document.base, last.node)
            {
                let last_text = String::from(parsed.document.character_data(last.node));
                let old_value = DomString::from(last_text.clone());
                let mut data = last_text;
                data.push_str(&String::from(parsed.document.character_data(next.node)));
                parsed
                    .document
                    .base
                    .mutate()
                    .set_node_text(last.node, &data);
                parsed.document.record(JournalEntry::CharacterData {
                    target: last,
                    old_value,
                });
                unlink_journaled(&mut parsed, next);
            }
        }
        drop(parsed);
        drop(world);
        world_for_node(&ctx, self.handle.0)?
            .borrow()
            .destroy_child_navigables(&iframes);
        fixup_focus_after_removal(&ctx, self.handle.0)?;
        schedule_mutation_delivery(&ctx)
    }

    /// Setting `innerText` replaces all children with the rendered text
    /// fragment: newlines become `br` elements with no empty text around
    /// them (an all-empty value yields one empty text node)
    /// (<https://html.spec.whatwg.org/multipage/dom.html#set-the-inner-text-steps>).
    #[qjs(set, rename = "innerText")]
    fn set_inner_text(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let html = world(&ctx)?
            .borrow()
            .document(self.handle.0)
            .is_some_and(|parsed| {
                parsed
                    .document
                    .base
                    .get_node(self.handle.0.node)
                    .is_some_and(|node| {
                        node.data
                            .downcast_element()
                            .is_some_and(|element| element.name.ns == html_namespace())
                    })
            });
        if !html {
            return Ok(());
        }
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let added = crate::dom::text::rendered_text_fragment(&mut parsed, &value.0, false);
        let iframes = replace_all_journaled(&mut parsed, self.handle.0, added);
        drop(parsed);
        world.destroy_child_navigables(&iframes);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // ── Element identity and attributes ─────────────────────────────────
}

// https://dom.spec.whatwg.org/#dom-nonelementparentnode-getelementbyid

// Empty element contracts: every member the generator lowers from these
// interfaces is fully generated (`[Reflect]` string/boolean/URL and
// `[ReflectSetter]` with an implemented getter), so the trait carries no
// methods yet. Adding the contract installs that generated surface;
// members needing hand-written algorithms stay absent until implemented.
