//! The bitmap surface shared by `HTMLCanvasElement` and `OffscreenCanvas`.
//!
//! Both platform objects draw into one registry slot keyed by [`CanvasId`].
//! Only dimensions are host-specific: a `<canvas>` reads its content
//! attributes, while an `OffscreenCanvas` stores them on its bitmap. The
//! context, snapshot, origin-clean flag, and open layers are all state of the
//! bitmap, so both hosts answer those through the same registry calls.
//!
//! Spec:
//! <https://html.spec.whatwg.org/multipage/canvas.html#the-canvas-element>,
//! <https://html.spec.whatwg.org/multipage/canvas.html#the-offscreencanvas-interface>

use rquickjs::{Ctx, Value};

use crate::dom::tree::{NodeId, is_html_tag};
use crate::js::bindings::{host_node_id, world};

use super::SourceImage;

/// An opaque bitmap id. It names the pixels, not a DOM node, so an
/// `OffscreenCanvas` needs no element and a stale id can never reach a later
/// canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CanvasId(pub(crate) u64);

/// The operations every canvas surface answers the same way.
pub(crate) trait CanvasHost {
    /// The bitmap this host draws into, or `None` before a context exists.
    fn canvas_id(&self, ctx: &Ctx<'_>) -> Option<CanvasId>;

    /// The bitmap's current dimensions.
    fn dimensions(&self, ctx: &Ctx<'_>) -> (u32, u32);

    /// A straight-RGBA copy of the bitmap, or `None` before a context exists.
    fn snapshot(&self, ctx: &Ctx<'_>) -> Option<SourceImage>;

    /// The origin-clean flag, or `None` before a context exists.
    fn origin_clean(&self, ctx: &Ctx<'_>) -> Option<bool>;

    /// Whether a `beginLayer()` is still open.
    fn has_open_layer(&self, ctx: &Ctx<'_>) -> bool;
}

/// A `<canvas>` element, whose dimensions reflect its content attributes
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-canvas-width>).
#[derive(Clone, Copy)]
pub(crate) struct ElementCanvas {
    pub(crate) node: NodeId,
}

impl ElementCanvas {
    /// Resolve a JS value that is a `<canvas>` this realm created. Another
    /// frame's element is not a source: its bitmap belongs to that frame.
    pub(crate) fn resolve<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<Self> {
        let node = host_node_id(ctx, value)?;
        let world = world(ctx).ok()?;
        let owner = world.borrow();
        if !owner.owns_node(node) {
            return None;
        }
        let is_canvas = owner
            .with_document(node, |parsed| {
                parsed
                    .document
                    .base
                    .get_node(node.node)
                    .is_some_and(|node| is_html_tag(Some(&node.data), "canvas"))
            })
            .unwrap_or(false);
        is_canvas.then_some(Self { node })
    }
}

impl CanvasHost for ElementCanvas {
    fn canvas_id(&self, ctx: &Ctx<'_>) -> Option<CanvasId> {
        let world = world(ctx).ok()?;
        let registry = world.borrow().registry();
        registry.borrow().canvas_id_for_node(self.node)
    }

    fn dimensions(&self, ctx: &Ctx<'_>) -> (u32, u32) {
        (
            super::canvas_dimension(ctx, self.node, "width", 300),
            super::canvas_dimension(ctx, self.node, "height", 150),
        )
    }

    fn snapshot(&self, ctx: &Ctx<'_>) -> Option<SourceImage> {
        let id = self.canvas_id(ctx)?;
        let world = world(ctx).ok()?;
        let registry = world.borrow().registry();
        registry.borrow().canvas_snapshot(id)
    }

    fn origin_clean(&self, ctx: &Ctx<'_>) -> Option<bool> {
        let id = self.canvas_id(ctx)?;
        let world = world(ctx).ok()?;
        let registry = world.borrow().registry();
        registry.borrow().canvas_origin_clean(id)
    }

    fn has_open_layer(&self, ctx: &Ctx<'_>) -> bool {
        let Some(id) = self.canvas_id(ctx) else {
            return false;
        };
        let Ok(world) = world(ctx) else {
            return false;
        };
        let registry = world.borrow().registry();
        registry.borrow().canvas_has_open_layer(id)
    }
}

/// An `OffscreenCanvas`, whose dimensions live on its bitmap
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-offscreencanvas-width>).
#[derive(Clone, Copy)]
pub(crate) struct OffscreenCanvasHost {
    pub(crate) id: CanvasId,
}

impl CanvasHost for OffscreenCanvasHost {
    fn canvas_id(&self, _ctx: &Ctx<'_>) -> Option<CanvasId> {
        Some(self.id)
    }

    fn dimensions(&self, ctx: &Ctx<'_>) -> (u32, u32) {
        let Ok(world) = world(ctx) else {
            return (0, 0);
        };
        let registry = world.borrow().registry();
        registry
            .borrow()
            .canvas_dimensions(self.id)
            .unwrap_or((0, 0))
    }

    fn snapshot(&self, ctx: &Ctx<'_>) -> Option<SourceImage> {
        let world = world(ctx).ok()?;
        let registry = world.borrow().registry();
        registry.borrow().canvas_snapshot(self.id)
    }

    fn origin_clean(&self, ctx: &Ctx<'_>) -> Option<bool> {
        let world = world(ctx).ok()?;
        let registry = world.borrow().registry();
        registry.borrow().canvas_origin_clean(self.id)
    }

    fn has_open_layer(&self, ctx: &Ctx<'_>) -> bool {
        let Ok(world) = world(ctx) else {
            return false;
        };
        let registry = world.borrow().registry();
        registry.borrow().canvas_has_open_layer(self.id)
    }
}
