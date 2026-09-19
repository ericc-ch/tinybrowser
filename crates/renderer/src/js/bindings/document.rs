//! Document predicates and URL helpers.

use super::{world, world_for_node};

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
    let Ok(owner) = world_for_node(ctx, id) else {
        return "about:blank".to_owned();
    };
    let world = owner.borrow();
    // Script-created documents carry their own URL (a DOMParser result takes
    // the realm's URL); everything else falls back to the world's document.
    if let Some(url) = world.document(id).and_then(|parsed| parsed.url.clone()) {
        return url;
    }
    if world.is_main_document(id) {
        return world.document_url.as_str().to_owned();
    }
    "about:blank".to_owned()
}

/// The document's base URL: the first `base` element's `href` resolved
/// against the document URL, or the document URL itself
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#document-base-url>).
pub(crate) fn document_base_url_string(ctx: &Ctx<'_>, id: NodeId) -> String {
    let fallback = document_url_string(ctx, id);
    let Ok(owner) = world_for_node(ctx, id) else {
        return fallback;
    };
    let world = owner.borrow();
    let Some(parsed) = world.document(id) else {
        return fallback;
    };
    let Some(base) = parsed
        .dom
        .select_all(parsed.dom.document(), "base")
        .ok()
        // Frozen base URL: the first `base` element *with* an `href`
        // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#document-base-url>).
        .and_then(|candidates| {
            candidates
                .into_iter()
                .find(|candidate| parsed.dom.attribute(*candidate, "href").is_some())
        })
    else {
        return fallback;
    };
    let Some(href) = parsed.dom.attribute(base, "href") else {
        return fallback;
    };
    url::Url::parse(&fallback)
        .ok()
        .and_then(|url| url.join(&href).ok())
        .map_or(fallback, |url| url.to_string())
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
