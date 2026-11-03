//! Document predicates and URL helpers.

use super::{world, world_for_node};
use crate::dom::tree::NodeId;

use rquickjs::Ctx;

/// Whether the document that owns `id` implements `XMLDocument`
/// (<https://dom.spec.whatwg.org/#xmldocument>).
pub(crate) fn document_is_xml_document(ctx: &Ctx<'_>, id: NodeId) -> bool {
    let Ok(world_rc) = world(ctx) else {
        return false;
    };
    let world = world_rc.borrow();
    world.document(id).is_some_and(|parsed| parsed.xml_document)
}

/// Whether the document that owns `id` reports `text/html`.
pub(crate) fn document_is_html_content(ctx: &Ctx<'_>, id: NodeId) -> bool {
    let Ok(world_rc) = world(ctx) else {
        return false;
    };
    let world = world_rc.borrow();
    world
        .document(id)
        .is_some_and(|parsed| parsed.content_type == "text/html")
}

/// Whether `id` is the realm's active document. See
/// [`World::is_main_document`](crate::js::world::World::is_main_document).
pub(crate) fn is_main_document(ctx: &Ctx<'_>, id: NodeId) -> bool {
    world(ctx).is_ok_and(|world| world.borrow().is_main_document(id))
}

pub(crate) fn document_url_string(ctx: &Ctx<'_>, id: NodeId) -> String {
    let Ok(owner) = world_for_node(ctx, id) else {
        return "about:blank".to_owned();
    };
    owner.borrow().document_url_of(id)
}

/// The document's own referrer for `document.referrer`. Only the frame's
/// active document has one: a document a script created (a `DOMParser`
/// result, a template's inert document) was never navigated to, so it reads
/// the empty string
/// (<https://html.spec.whatwg.org/multipage/dom.html#dom-document-referrer>).
pub(crate) fn document_referrer_string(ctx: &Ctx<'_>, id: NodeId) -> String {
    let Ok(owner) = world_for_node(ctx, id) else {
        return String::new();
    };
    let world = owner.borrow();
    if !world.is_main_document(id) {
        return String::new();
    }
    world
        .document_referrer
        .as_ref()
        .map_or_else(String::new, ToString::to_string)
}

/// The document's base URL: the first `base` element's `href` resolved
/// against the fallback base URL, or the fallback itself
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#document-base-url>).
pub(crate) fn document_base_url_string(ctx: &Ctx<'_>, id: NodeId) -> String {
    let Ok(owner) = world_for_node(ctx, id) else {
        return document_url_string(ctx, id);
    };
    owner.borrow().document_base_url_of(id)
}
/// Whether the document that owns `id` is an HTML document
/// (`text/html` or `application/xhtml+xml`).
pub(crate) fn document_is_html(ctx: &Ctx<'_>, id: NodeId) -> bool {
    let Ok(world_rc) = world(ctx) else {
        return false;
    };
    let world = world_rc.borrow();
    world
        .document(id)
        .is_some_and(|parsed| matches!(parsed.content_type, "text/html" | "application/xhtml+xml"))
}
