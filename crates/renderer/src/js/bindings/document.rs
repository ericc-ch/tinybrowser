//! Document predicates and URL helpers.

use super::world;

use dom::{NodeId, NodeKind, html_namespace};

use rquickjs::Ctx;

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
/// [`World::is_main_document`].
pub(crate) fn is_main_document(ctx: &Ctx<'_>, id: NodeId) -> bool {
    world(ctx).is_ok_and(|world| world.borrow().is_main_document(id))
}

pub(crate) fn document_url_string(ctx: &Ctx<'_>, id: NodeId) -> String {
    if is_main_document(ctx, id) {
        return world(ctx)
            .map(|world| world.borrow().document_url.as_str().to_owned())
            .unwrap_or_default();
    }
    "about:blank".to_owned()
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

/// Whether `id` names an element in the HTML namespace.
pub(crate) fn element_is_html(ctx: &Ctx<'_>, id: NodeId) -> bool {
    let Ok(world) = world(ctx) else {
        return false;
    };
    let parsed = world.borrow();
    parsed.with_document(id, |parsed| {
        matches!(
            parsed.dom.kind(id),
            Some(NodeKind::Element { name, .. }) if name.ns == html_namespace()
        )
    }) == Some(true)
}
