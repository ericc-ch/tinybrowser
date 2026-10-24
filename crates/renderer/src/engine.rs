//! The page engine for one renderer process: the shared `QuickJS` heap, the Tokio
//! waiter, and the frame registry.
//!
//! One `QuickJS` `Runtime` and one Tokio waiter per renderer process;
//! `Document` is one frame. Child frames join the same engine sharing both,
//! while each keeps its own realm: JS values never cross realms, so the
//! engine routes serialized payloads and owns the channel endpoints
//! ([`crate::messaging`]).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;
use url::Url;

use crate::RemoteValue;
use crate::document::{Document, WindowMessage};
use crate::js::FrameRuntime;
use crate::js::Stop;

/// The engine's default frame viewport in device pixels, used until CDP
/// emulation (or a mount) sets another size.
pub const DEFAULT_VIEWPORT: (u32, u32) = (800, 600);

use crate::documents::DocumentStore;
use crate::js::{FrameNavigation, NavigationTarget, RealmRegistry, SharedJsRuntime};
use crate::messaging::{Delivery, SharedHandle, TargetOrigin};
use crate::protocol::{
    BrowserEvent, BrowserServices, FrameId, Mount, RendererEvent, ResponseHead, TabError,
};
use crate::storage::PendingStorageEvent;

/// One renderer process's page engine.
///
/// The main frame is the tab's top-level document; child frames share the
/// engine's heap and wake handle, so same-site frames can pass JavaScript
/// objects synchronously.
pub struct Engine {
    /// The handles every frame document shares.
    runtime: FrameRuntime,
    frames: BTreeMap<FrameId, Document>,
    js_runtime: SharedJsRuntime,
    /// Page-scoped init scripts shared by every frame's world, with their
    /// identifiers (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-addScriptToEvaluateOnNewDocument>).
    init_scripts: Rc<RefCell<Vec<(u64, String)>>>,
}

impl Engine {
    /// Builds an engine whose effects go through `services`.
    ///
    /// `stop` cancels every frame at once; `wake` is notified when a dial
    /// completion or stop arrives, so a carrier can wait instead of polling.
    #[must_use]
    pub fn new(services: Arc<dyn BrowserServices>, stop: Arc<Stop>, wake: Arc<Notify>) -> Self {
        let js_runtime = SharedJsRuntime::default();
        let init_scripts: Rc<RefCell<Vec<(u64, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let runtime = FrameRuntime {
            services,
            js_runtime: js_runtime.handle(),
            wake,
            stop,
            documents: Rc::new(RefCell::new(DocumentStore::default())),
            registry: Rc::new(RefCell::new(RealmRegistry::default())),
            shared: Rc::new(RefCell::new(crate::messaging::Shared::default())),
            pending_storage: Rc::new(RefCell::new(Vec::new())),
            turn_deadline: Rc::new(std::cell::Cell::new(None)),
            font_ctx: parley::FontContext::default(),
        };
        let main = Document::with_shared(FrameId::MAIN, &runtime, &init_scripts);
        let mut frames = BTreeMap::new();
        frames.insert(FrameId::MAIN, main);
        Self {
            runtime,
            frames,
            js_runtime,
            init_scripts,
        }
    }

    /// Adopts every child frame the realms created for themselves (an `iframe`
    /// insertion inside a script) and starts any `src` load it still needs.
    ///
    /// Returns whether any frame was adopted, so the reconcile loop runs again
    /// before the engine sleeps.
    fn adopt_pending_frames(&mut self) -> bool {
        // Frames are materialized in tree order with scripts. Scanning every
        // document here would start parser `src` loads too early.
        let mut adopted = false;
        let mut batch = Vec::new();
        loop {
            for (&parent, document) in &mut self.frames {
                for (child, container, document) in document.take_new_frames() {
                    batch.push((parent, child, container, document));
                }
            }
            if batch.is_empty() {
                return adopted;
            }
            adopted = true;
            for (parent, child, container, document) in batch.drain(..) {
                self.frames.insert(child, document);
                // `srcdoc` takes priority over `src`
                // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-srcdoc>).
                let srcdoc = self
                    .frames
                    .get(&parent)
                    .and_then(|document| document.frame_srcdoc(container));
                if let Some(markup) = srcdoc {
                    let navigation =
                        FrameNavigation::srcdoc(NavigationTarget::Container(container), markup);
                    self.navigate_frame(child, parent, &navigation);
                } else {
                    let src = self
                        .frames
                        .get(&parent)
                        .and_then(|document| document.frame_src(container));
                    if let Some(src) = src {
                        let navigation =
                            FrameNavigation::get(NavigationTarget::Container(container), src);
                        self.navigate_frame(child, parent, &navigation);
                    }
                }
                if let Some(document) = self.frames.get_mut(&parent) {
                    document.mark_frame_load_pending(container);
                    document.flush_frame_proxy_sets(child);
                }
                self.publish_frame_document(container, child);
            }
        }
    }

    /// The document of one frame.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host `frame`.
    fn frame_mut(&mut self, frame: FrameId) -> Result<&mut Document, TabError> {
        self.frames
            .get_mut(&frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })
    }

    /// Replaces one frame's document from a host mount.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host `frame`.
    pub fn mount_frame(&mut self, frame: FrameId, mount: &Mount) -> Result<(), TabError> {
        // A main-frame mount replaces the page-scoped init scripts before the
        // document's realm can run, so the fresh tree sees the current list.
        if frame == FrameId::MAIN {
            self.init_scripts
                .borrow_mut()
                .clone_from(&mount.head.init_scripts);
        }
        self.remove_descendants(frame);
        self.frame_mut(frame)?.mount(mount)?;
        self.reconcile_frames();
        Ok(())
    }

    /// Opens a response body for `frame`, whose bytes arrive through
    /// [`Engine::write_body`] until [`Engine::end_body`].
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host the frame.
    pub fn open_body(&mut self, frame: FrameId, head: &ResponseHead) -> Result<(), TabError> {
        if frame == FrameId::MAIN {
            self.init_scripts
                .borrow_mut()
                .clone_from(&head.init_scripts);
        }
        self.remove_descendants(frame);
        let document = self.frame_mut(frame)?;
        document.set_history(&head.history);
        document.install_head(head, None);
        Ok(())
    }

    /// Feeds more response bytes to a frame opened by [`Engine::open_body`].
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host the frame.
    pub fn write_body(&mut self, frame: FrameId, bytes: &[u8]) -> Result<(), TabError> {
        self.frame_mut(frame)?.write_body(bytes);
        self.reconcile_frames();
        Ok(())
    }

    /// Ends a response body opened by [`Engine::open_body`].
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host the frame.
    pub fn end_body(&mut self, frame: FrameId) -> Result<(), TabError> {
        self.frame_mut(frame)?.end_body();
        self.reconcile_frames();
        Ok(())
    }

    /// Abandons a response body that will not be finished.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host the frame.
    pub fn abort_body(&mut self, frame: FrameId) -> Result<(), TabError> {
        self.frame_mut(frame)?.abort_body();
        self.reconcile_frames();
        Ok(())
    }

    /// Evaluates `source` in one frame and returns its string coercion.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] or [`TabError::Script`].
    pub fn eval_in(&mut self, frame: FrameId, source: &str) -> Result<String, TabError> {
        let result = self.frame_mut(frame)?.eval(source);
        self.reconcile_frames();
        result
    }

    /// Evaluates `source` in one frame and interns node handles for the protocol.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] or [`TabError::Script`].
    pub fn execute_remote_in(
        &mut self,
        frame: FrameId,
        source: crate::ScriptSource<&str>,
        timeout: Option<Duration>,
    ) -> Result<RemoteValue, TabError> {
        let result = self.frame_mut(frame)?.execute_remote(source, timeout);
        self.reconcile_frames();
        result
    }

    /// Sets `frame`'s persistent viewport and re-lays out its active document.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host `frame`.
    pub fn set_viewport(
        &mut self,
        frame: FrameId,
        width: u32,
        height: u32,
    ) -> Result<(), TabError> {
        if frame == FrameId::MAIN {
            // Child frames share the tab's viewport; Blitz does not couple
            // iframe layout to the child, so the tab size is the correct
            // approximation until it does.
            let frames: Vec<FrameId> = self.frames.keys().copied().collect();
            for id in frames {
                if let Some(document) = self.frames.get_mut(&id) {
                    document.set_viewport(width, height);
                }
            }
            return Ok(());
        }
        self.frame_mut(frame)?.set_viewport(width, height);
        Ok(())
    }

    /// Registers a page-scoped init script for every frame's future realms
    /// and, when `run_immediately`, evaluates it in every existing frame now
    /// (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-addScriptToEvaluateOnNewDocument>).
    pub fn add_init_script(&mut self, id: u64, source: &str, run_immediately: bool) {
        self.init_scripts.borrow_mut().push((id, source.to_owned()));
        if !run_immediately {
            return;
        }
        let frames: Vec<FrameId> = self.frames.keys().copied().collect();
        for frame in frames {
            if let Some(document) = self.frames.get_mut(&frame) {
                document.run_init_script(source);
            }
        }
    }

    /// Removes a page-scoped init script.
    pub fn remove_init_script(&mut self, id: u64) {
        self.init_scripts
            .borrow_mut()
            .retain(|(script_id, _)| *script_id != id);
    }

    /// The frame's scrollable content size in CSS pixels.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host `frame`.
    pub fn content_size(&mut self, frame: FrameId) -> Result<(f32, f32), TabError> {
        Ok(self.frame_mut(frame)?.content_size())
    }

    /// Renders one frame to a PNG.
    ///
    /// The document's `<style>` sheets are applied. The viewport and crop
    /// window come from the caller (Playwright's `clip`); the base viewport
    /// is the shared 800x600 virtual size.
    ///
    /// Viewport sides convert saturating at least 1 (the painter rejects
    /// over-cap sizes).
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the frame is not mounted and
    /// [`TabError::Render`] when the pipeline refuses the document.
    pub fn screenshot_frame(
        &mut self,
        frame: FrameId,
        request: &crate::ScreenshotRequest,
    ) -> Result<Vec<u8>, TabError> {
        let document = self
            .frames
            .get(&frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?;
        let world = document.world();
        let image = {
            let world = world.borrow();
            let Some(mut parsed) = world.main_document_mut() else {
                return Err(TabError::RendererUnavailable {
                    message: "no document to render".into(),
                });
            };
            // The capture surface may exceed the persistent viewport (a clip
            // or full-page shot), so it applies for this capture only and is
            // restored afterwards, keeping layout in step with `window`
            // metrics
            // (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-captureScreenshot>).
            let (width, height) = capture_sides(request.viewport_width, request.viewport_height)?;
            let previous = parsed.document.base.viewport().clone();
            parsed
                .document
                .base
                .set_viewport(blitz_traits::shell::Viewport::new(
                    width,
                    height,
                    1.0,
                    blitz_traits::shell::ColorScheme::Light,
                ));
            crate::render::resolve_until_settled(&mut parsed.document.base);
            let image = crate::render::paint(&mut parsed.document.base, width, height).map_err(
                |error| TabError::Render {
                    message: error.to_string(),
                },
            )?;
            parsed.document.base.set_viewport(previous);
            image
        };
        let image = match request.clip {
            Some(clip) => image
                .crop(clip.x, clip.y, clip.width, clip.height)
                .map_err(|error| TabError::Render {
                    message: error.to_string(),
                })?,
            None => image,
        };
        crate::render::encode_png(&image).map_err(|error| TabError::Render {
            message: error.to_string(),
        })
    }

    /// Removes and returns every pending event, with its frame.
    ///
    /// # Errors
    ///
    /// [`TabError::RendererUnavailable`] when a frame retained more events
    /// than its limit; the queue empties and the excess is lost.
    pub fn take_events(&mut self) -> Result<Vec<(FrameId, RendererEvent)>, TabError> {
        let mut events = Vec::new();
        for (&frame, document) in &mut self.frames {
            let pending = document
                .take_events()
                .map_err(|()| TabError::RendererUnavailable {
                    message: "the retained event limit was exceeded".into(),
                })?;
            events.extend(pending.into_iter().map(|event| (frame, event)));
        }
        Ok(events)
    }

    /// Removes and returns every pending event the browser acts on, with its
    /// frame. The flag reports whether renderer-internal progress was
    /// discarded: it carries no browser-side state, but the browser still
    /// needs a wake to re-check its JS-truth waiters.
    ///
    /// # Errors
    ///
    /// [`TabError::RendererUnavailable`] when a frame retained more events
    /// than its limit; the queue empties and the excess is lost.
    pub fn take_browser_events(
        &mut self,
    ) -> Result<(Vec<(FrameId, BrowserEvent)>, bool), TabError> {
        let mut internal = false;
        let mut events = Vec::new();
        for (frame, event) in self.take_events()? {
            match event.into_browser() {
                Some(event) => events.push((frame, event)),
                None => internal = true,
            }
        }
        Ok((events, internal))
    }

    /// Runs every immediately ready task in every frame until the shared
    /// script deadline. Never blocks.
    ///
    /// Returns whether ready work was left queued because the deadline passed.
    /// The session then reads its inbox before draining again, so a batch of
    /// runaway timers cannot hold the renderer past the request timeout.
    #[must_use]
    pub fn drain_ready(&mut self) -> bool {
        self.drain_ready_until(std::time::Instant::now() + crate::TURN_SCRIPT_BUDGET)
    }

    /// [`Engine::drain_ready`] bounded by `deadline`.
    ///
    /// Returns whether ready work remains queued after `deadline`.
    #[must_use]
    pub fn drain_ready_until(&mut self, deadline: std::time::Instant) -> bool {
        // Reconcile around every task so a task that inserts an iframe is
        // visible to the next task: `contentDocument` is non-null from the
        // task after the connection, not inside the connecting task itself
        // (https://html.spec.whatwg.org/multipage/iframe-embed-object.html#dom-iframe-contentdocument).
        //
        // Frames take one task per pass, round-robin: messages posted to
        // different frames must run in send order, and draining one frame dry
        // would let a later message overtake an earlier one
        // (<https://html.spec.whatwg.org/multipage/webappapis.html#task-queue>).
        //
        // One deadline covers the whole drain. Each task still has its own
        // entry budget, clamped to this instant, and a task that would start
        // after the instant stays queued.
        let _turn = self.runtime.open_turn(deadline);
        self.reconcile_frames();
        loop {
            if self.runtime.stop.is_set() {
                return false;
            }
            if std::time::Instant::now() >= deadline {
                return self.has_ready_work();
            }
            self.drain_storage_events();
            let mut ran = false;
            let frames: Vec<FrameId> = self.frames.keys().copied().collect();
            for frame in frames {
                if std::time::Instant::now() >= deadline {
                    return self.has_ready_work();
                }
                let more = match self.frames.get_mut(&frame) {
                    Some(document) => document.drain_step(),
                    None => false,
                };
                if more {
                    ran = true;
                    self.reconcile_frames();
                }
            }
            if !ran {
                break;
            }
        }
        false
    }

    fn has_ready_work(&self) -> bool {
        self.frames.values().any(Document::has_ready_work)
    }

    /// Queues one remote `postMessage` payload on the main frame's task
    /// source; the frame dispatches a trusted `message` event with a null
    /// source, after checking the intended target origin against whichever
    /// document runs the task. `/` names the sender's origin, which travels
    /// with the message because only its serialization survives the browser
    /// hop.
    pub fn receive_remote_window_message(
        &mut self,
        origin: String,
        target_origin: &str,
        payload: String,
    ) {
        let Some(target_origin) = TargetOrigin::from_remote(target_origin, &origin) else {
            return;
        };
        if let Some(document) = self.frames.get_mut(&FrameId::MAIN) {
            document.push_remote_message(origin, target_origin, payload);
        }
    }

    /// Activates an entry that belongs to the main frame's current document.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the main frame is not mounted and
    /// [`TabError::InvalidUrl`] when `url` does not parse or is cross-origin.
    pub fn traverse_history(
        &mut self,
        url: &str,
        history: &crate::protocol::HistorySnapshot,
    ) -> Result<(), TabError> {
        let document = self
            .frames
            .get_mut(&FrameId::MAIN)
            .ok_or(TabError::UnknownFrame {
                frame: FrameId::MAIN.get(),
            })?;
        document.traverse_history(url, history)
    }

    /// Queues one `BroadcastChannel` message on every same-origin frame of
    /// this engine
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#broadcasting-to-other-browsing-contexts>).
    pub fn receive_broadcast_message(
        &mut self,
        origin: &str,
        name: &str,
        payload: &str,
        source: Option<u64>,
    ) {
        for document in self.frames.values_mut() {
            if document.origin_string() != origin {
                continue;
            }
            if !document.world().borrow().is_attached() {
                continue;
            }
            document.push_broadcast_message(
                origin.to_owned(),
                name.to_owned(),
                payload.to_owned(),
                source,
            );
        }
    }

    /// Queues one `localStorage` change that the browser broadcast. The source
    /// frame is set only when the change came from this same assignment;
    /// other renderers and assignments pass `None`.
    pub fn receive_storage_event(&mut self, event: PendingStorageEvent) {
        self.runtime.pending_storage.borrow_mut().push(event);
    }

    /// Moves changes queued by this engine's own script onto the
    /// `storage`-event task of every receiving frame.
    fn drain_storage_events(&mut self) {
        let pending: Vec<PendingStorageEvent> =
            std::mem::take(&mut *self.runtime.pending_storage.borrow_mut());
        for event in pending {
            self.queue_storage_event(&event);
        }
    }

    /// Queues one change on every frame of `event.origin`, excluding the
    /// window whose script made it
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
    fn queue_storage_event(&mut self, event: &PendingStorageEvent) {
        let frames: Vec<FrameId> = self.frames.keys().copied().collect();
        for frame in frames {
            if event.source == Some(frame) {
                continue;
            }
            let Some(document) = self.frames.get_mut(&frame) else {
                continue;
            };
            if document.origin_string() != event.origin {
                continue;
            }
            document.push_storage_event(event.clone());
        }
    }

    /// Earliest timer deadline across every frame.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.frames
            .values()
            .filter_map(Document::next_deadline)
            .min()
    }

    fn reconcile_frames(&mut self) {
        loop {
            let adopted = self.adopt_pending_frames();
            let removed = self.runtime.shared.borrow_mut().take_removed_frames();
            for (frame, container) in &removed {
                if let Some(container) = container {
                    self.runtime.registry.borrow_mut().forget_frame(*container);
                }
                self.runtime
                    .registry
                    .borrow_mut()
                    .forget_frame_world(*frame);
                self.frames.remove(frame);
            }
            let frame_ids: Vec<FrameId> = self.frames.keys().copied().collect();
            let mut iframe_lifecycle = Vec::new();
            let mut image_lifecycle = Vec::new();
            let mut navigations = Vec::new();
            let mut streams = Vec::new();
            for frame in frame_ids {
                let Some(document) = self.frames.get_mut(&frame) else {
                    continue;
                };
                let (iframes, images) = document.take_lifecycle();
                iframe_lifecycle.extend(iframes.into_iter().map(|event| (frame, event)));
                image_lifecycle.extend(images.into_iter().map(|event| (frame, event)));
                navigations.extend(
                    document
                        .take_frame_navigations()
                        .into_iter()
                        .map(|navigation| (frame, navigation)),
                );
                if let Some(closed) = document.take_stream_publish() {
                    streams.push((frame, closed));
                }
            }
            let deliveries = self.runtime.shared.borrow_mut().take_deliveries();
            let had_work = adopted
                || !iframe_lifecycle.is_empty()
                || !image_lifecycle.is_empty()
                || !navigations.is_empty()
                || !streams.is_empty()
                || !deliveries.is_empty();
            self.apply_iframe_lifecycle(iframe_lifecycle);
            self.apply_image_lifecycle(image_lifecycle);
            self.apply_navigations(navigations);
            self.publish_streams(streams);
            self.apply_deliveries(deliveries);
            self.flush_embedder_csp();
            let fired_load = self.fire_ready_frame_loads();
            if !had_work && !fired_load {
                break;
            }
        }
    }

    /// Applies iframe connection transitions.
    fn apply_iframe_lifecycle(&mut self, events: Vec<(FrameId, crate::document::IframeLifecycle)>) {
        for (_parent, event) in events {
            match event {
                crate::document::IframeLifecycle::Removed(container) => {
                    self.remove_subtree(container);
                }
            }
        }
    }

    /// Applies image connection transitions.
    fn apply_image_lifecycle(&mut self, events: Vec<(FrameId, crate::document::ImageLifecycle)>) {
        for (parent, event) in events {
            match event {
                crate::document::ImageLifecycle::Connected(element) => {
                    if let Some(document) = self.frames.get_mut(&parent) {
                        document.queue_connected_image(element);
                    }
                }
                crate::document::ImageLifecycle::Disconnected(element) => {
                    if let Some(document) = self.frames.get_mut(&parent) {
                        document.disconnect_image(element);
                    }
                }
            }
        }
    }

    /// Removes a frame and every descendant it owns before the backing
    /// document is replaced or disconnected. Lifecycle events can arrive only
    /// for the direct iframe, so recurse through the frame tree explicitly to
    /// avoid stale realms and pending loads.
    fn remove_subtree(&mut self, container: crate::dom::tree::NodeId) {
        let Some(child) = self
            .runtime
            .shared
            .borrow()
            .tree
            .frame_for_container(container)
        else {
            return;
        };
        let descendants: Vec<crate::dom::tree::NodeId> = self
            .runtime
            .shared
            .borrow()
            .tree
            .children(child)
            .iter()
            .filter_map(|&nested| self.runtime.shared.borrow().tree.container(nested))
            .collect();
        for nested in descendants {
            self.remove_subtree(nested);
        }
        if let Some(parent) = self.runtime.shared.borrow().tree.parent(child)
            && let Some(document) = self.frames.get_mut(&parent)
        {
            document.cancel_frame_load(container);
        }
        self.runtime.registry.borrow_mut().forget_frame(container);
        self.runtime.registry.borrow_mut().forget_frame_world(child);
        {
            let mut shared = self.runtime.shared.borrow_mut();
            shared.tree.remove(child);
            shared.forget_document(child);
        }
        // Dropping the document closes its ports and its realm.
        self.frames.remove(&child);
    }

    fn remove_descendants(&mut self, parent: FrameId) {
        let containers: Vec<crate::dom::tree::NodeId> = self
            .runtime
            .shared
            .borrow()
            .tree
            .children(parent)
            .iter()
            .filter_map(|&child| self.runtime.shared.borrow().tree.container(child))
            .collect();
        for container in containers {
            self.remove_subtree(container);
        }
    }

    /// Navigates `child` to one URL, resolving a relative URL against the
    /// document that queued the navigation and dispatching on its scheme
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    ///
    /// The embedder is read from the tree: it is the initiator for every
    /// navigation a fresh `iframe` starts, and only a link that targets another
    /// navigable splits the two.
    fn navigate_frame(&mut self, child: FrameId, initiator: FrameId, navigation: &FrameNavigation) {
        let (embedder, container) = {
            let shared = self.runtime.shared.borrow();
            (shared.tree.parent(child), shared.tree.container(child))
        };
        // A `srcdoc` attribute takes priority over `src`: the frame navigates
        // to `about:srcdoc` with the attribute's value as the document source
        // (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
        if let Some(markup) = &navigation.srcdoc {
            self.load_frame_srcdoc(child, initiator, markup);
            if let (Some(embedder), Some(container)) = (embedder, container) {
                self.mark_frame_load_pending(container, embedder);
            }
            return;
        }
        // An empty `src` is a container with no URL to load; it keeps its
        // `about:blank`. Every other navigation resolves an empty string
        // against the initiator document, which is that document's own URL
        // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#parse-a-url>),
        // so a link or form that names no URL reloads its document instead of
        // blanking it.
        let blank = navigation.spec.is_empty()
            && matches!(navigation.target, NavigationTarget::Container(_));
        let resolved = (!blank)
            .then(|| {
                self.frames
                    .get(&initiator)
                    .and_then(|document| document.resolve_frame_url(&navigation.spec))
            })
            .flatten();
        let Some(url) = resolved else {
            if blank {
                // About:blank, the initial document of every frame without a
                // usable `src`, inherits the initiator's captured origin
                // (<https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>).
                // A frame the engine just created is already on that document;
                // reloading it would throw away its realm and anything a script
                // has already set on it.
                self.keep_initial_blank(child, initiator);
            }
            if let (Some(embedder), Some(container)) = (embedder, container) {
                self.mark_frame_load_pending(container, embedder);
            }
            return;
        };
        self.load_frame_url(child, &url, initiator, navigation);
        if let (Some(embedder), Some(container)) = (embedder, container) {
            self.mark_frame_load_pending(container, embedder);
        }
    }

    /// The referrer a navigation sends: the navigating element's policy when it
    /// names one, the initiator document's otherwise
    /// (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>,
    /// <https://html.spec.whatwg.org/multipage/links.html#hyperlink-referrer-policy>).
    fn navigation_referrer(&self, initiator: FrameId, policy: &str, url: &Url) -> Option<String> {
        self.frames.get(&initiator).and_then(|document| {
            let world = document.world();
            let world = world.borrow();
            let policy = if policy.is_empty() {
                world.state.referrer_policy.as_str()
            } else {
                policy
            };
            crate::js::referrer_for_policy(policy, &world.document_url, url.as_str())
        })
    }

    /// The policy a local-scheme document inherits from its initiator
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#determining-navigation-params-policy-container>).
    fn inherited_policy(&self, parent: FrameId) -> crate::csp::List {
        self.frames
            .get(&parent)
            .map(|document| document.world().borrow().csp.clone())
            .unwrap_or_default()
    }

    /// Gives a frame's freshly installed local-scheme document the
    /// initiator's policy.
    fn inherit_policy(&self, child: FrameId, parent: FrameId) {
        let policy = self.inherited_policy(parent);
        if let Some(document) = self.frames.get(&child) {
            document.world().borrow_mut().csp = policy;
        }
    }

    /// Keeps a frame's initial `about:blank` document while it still has one,
    /// so a script-set realm survives; otherwise installs the inherited blank.
    fn keep_initial_blank(&mut self, child: FrameId, parent: FrameId) {
        let inherited = self.frames.get(&parent).map(|document| {
            (
                document.inherited_url(),
                document.world().borrow().origin.clone(),
            )
        });
        if self
            .frames
            .get(&child)
            .is_some_and(Document::is_initial_blank)
        {
            self.inherit_policy(child, parent);
            return;
        }
        let Some(document) = self.frames.get_mut(&child) else {
            return;
        };
        match inherited {
            Some((url, origin)) => document.load_about_blank(Some(&url), Some(origin)),
            None => document.load_about_blank(Some("about:blank"), None),
        }
        self.inherit_policy(child, parent);
    }

    /// Whether `url` is a nested cross-document navigation that `frame-src`
    /// governs. Same-document navigations and `about:` are not.
    fn frame_src_applies(&self, child: FrameId, url: &Url) -> bool {
        if self.runtime.shared.borrow().tree.parent(child).is_none() {
            return false;
        }
        if !matches!(url.scheme(), "http" | "https" | "data" | "blob") {
            return false;
        }
        let Some(document) = self.frames.get(&child) else {
            return false;
        };
        let Ok(mut current) = Url::parse(document.document_url()) else {
            return true;
        };
        let mut next = url.clone();
        current.set_fragment(None);
        next.set_fragment(None);
        current != next
    }

    /// Ancestor document URLs nearest-first for a nested frame's `frame-ancestors`
    /// check at commit. Empty for a top-level navigation, which skips it
    /// (<https://w3c.github.io/webappsec-csp/#frame-ancestors-navigation-response>).
    fn ancestor_urls(&self, embedder: Option<FrameId>) -> Vec<Url> {
        let mut ancestors = Vec::new();
        let mut cursor = embedder;
        while let Some(frame) = cursor {
            let Some(document) = self.frames.get(&frame) else {
                break;
            };
            let Ok(url) = Url::parse(document.document_url()) else {
                break;
            };
            ancestors.push(url);
            cursor = self.runtime.shared.borrow().tree.parent(frame);
        }
        ancestors
    }

    /// Reports the embedder's `frame-src` violations and returns whether the
    /// enforced policy refuses `url`
    /// (<https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>).
    fn frame_src_blocked(&mut self, embedder: FrameId, child: FrameId, url: &Url) -> bool {
        let violations = {
            let Some(document) = self.frames.get(&embedder) else {
                return false;
            };
            let world = document.world();
            let world = world.borrow();
            world.csp.check_request(
                &world.origin,
                &world.document_url,
                url,
                crate::csp::Destination::Frame,
                None,
            )
        };
        if violations.is_empty() {
            return false;
        }
        let container = self.runtime.shared.borrow().tree.container(child);
        if let Some(document) = self.frames.get_mut(&embedder) {
            let target = container.or_else(|| document.document_node());
            if let Some(target) = target {
                document.fire_csp_violations(target, &violations);
            }
        }
        crate::csp::blocks(&violations)
    }

    /// Dispatches `frame-src` redirect violations into the embedding document.
    fn flush_embedder_csp(&mut self) {
        let frames: Vec<FrameId> = self.frames.keys().copied().collect();
        let mut reports = Vec::new();
        for frame in frames {
            if let Some(document) = self.frames.get_mut(&frame) {
                reports.extend(document.take_embedder_csp_reports());
            }
        }
        for (embedder, violations) in reports {
            let Some(document) = self.frames.get_mut(&embedder) else {
                continue;
            };
            let Some(target) = document.document_node() else {
                continue;
            };
            document.fire_csp_violations(target, &violations);
        }
    }

    /// Starts the load one resolved frame URL names, dispatching on its scheme
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    fn load_frame_url(
        &mut self,
        child: FrameId,
        url: &Url,
        initiator: FrameId,
        navigation: &FrameNavigation,
    ) {
        // Nested navigations are `frame-src` on the embedder. A same-document
        // navigation is not, and `about:` stays allowed: blocking it would
        // only show `about:blank` again
        // (<https://w3c.github.io/webappsec-csp/#effective-directive-for-a-request>).
        let embedder = self.runtime.shared.borrow().tree.parent(child);
        let check_frame_src = self.frame_src_applies(child, url);
        if check_frame_src
            && let Some(embedder) = embedder
            && self.frame_src_blocked(embedder, child, url)
        {
            return;
        }
        let frame_gate = (check_frame_src && matches!(url.scheme(), "http" | "https"))
            .then(|| {
                embedder.and_then(|embedder| {
                    self.frames
                        .get(&embedder)
                        .map(|document| document.redirect_gate(crate::csp::Destination::Frame))
                })
            })
            .flatten();
        match url.scheme() {
            "about" => self.keep_initial_blank(child, initiator),
            "data" => self.load_data_frame(child, initiator, url),
            "javascript" => self.load_javascript_frame_url(child, initiator, url),
            "blob" => self.load_blob_frame(child, initiator, url),
            "http" | "https" => {
                // The document the navigation started from, which supplies the
                // request's initiator.
                let source = self
                    .frames
                    .get(&initiator)
                    .and_then(|document| Url::parse(document.document_url()).ok())
                    .unwrap_or_else(|| url.clone());
                // Top-level navigations send the source document's referrer
                // like subresource requests do; without this every
                // navigation is a no-referrer navigation regardless of
                // policy
                // (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
                let referrer =
                    self.navigation_referrer(initiator, &navigation.referrer_policy, url);
                // The embedder's `frame-src` gate re-checks every redirect hop,
                // and the ancestor URLs back the child response's
                // `frame-ancestors` check at commit — both only when the
                // embedder's gate applies
                // (<https://w3c.github.io/webappsec-csp/#frame-ancestors-navigation-response>).
                let (embedder, gate, ancestors) =
                    if let (Some(embedder), Some(gate)) = (embedder, frame_gate) {
                        (
                            Some(embedder),
                            Some(gate),
                            self.ancestor_urls(Some(embedder)),
                        )
                    } else {
                        (None, None, Vec::new())
                    };
                if let Some(document) = self.frames.get_mut(&child) {
                    document.navigate(
                        embedder,
                        crate::protocol::DialRequest {
                            kind: crate::protocol::DialKind::FrameLoad,
                            url: url.to_string(),
                            initiator: source.to_string(),
                            // The frame's document always reads the body.
                            read_body: true,
                            method: navigation.method.clone(),
                            body: navigation.body.clone(),
                            content_type: navigation.content_type.clone(),
                            headers: Vec::new(),
                            referrer,
                            timeout_ms: 0,
                            csp: gate.map(Box::new),
                            cors: None,
                            keepalive: false,
                            body_len: 0,
                            integrity: None,
                            cache: None,
                            stream_body: false,
                            referrer_source: None,
                            referrer_policy: None,
                        },
                        ancestors,
                    );
                }
            }
            _ => {
                if let Some(document) = self.frames.get_mut(&child) {
                    document.load_about_blank(None, None);
                }
                self.inherit_policy(child, initiator);
            }
        }
    }

    /// Navigates to a `javascript:` URL returns when the initiator is not same
    /// origin-domain with the target's active document
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate-to-a-javascript:-url>).
    fn load_javascript_frame_url(&mut self, child: FrameId, initiator: FrameId, url: &Url) {
        let same_origin = match (self.frames.get(&initiator), self.frames.get(&child)) {
            (Some(source), Some(target)) => {
                source.world().borrow().origin == target.world().borrow().origin
            }
            _ => false,
        };
        if !same_origin {
            return;
        }
        let script = percent_decode(url.as_str());
        let script = script
            .strip_prefix("javascript:")
            .unwrap_or(&script)
            .to_owned();
        let source = url.as_str().to_owned();
        let initial = self
            .frames
            .get(&child)
            .is_some_and(Document::is_initial_blank);
        // The inline check for a `javascript:` navigation uses the initiator's
        // policy, not the target frame's
        // (<https://w3c.github.io/webappsec-csp/#should-block-navigation-request>).
        let policy = self
            .frames
            .get(&initiator)
            .map(|document| document.world().borrow().csp.clone())
            .unwrap_or_default();
        if let Some(document) = self.frames.get_mut(&child) {
            if initial {
                // The script runs in the document the browser already created;
                // a string result still replaces it.
                document.eval_frame_script(&script, &source, &policy);
            } else {
                document.load_javascript_frame(&script, &source, &policy);
            }
        }
    }

    /// Loads a frame's `about:srcdoc` document, inheriting the creator's base
    /// URL, origin, and policy container
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-srcdoc>).
    fn load_frame_srcdoc(&mut self, child: FrameId, parent: FrameId, markup: &str) {
        let inherited = self.frames.get(&parent).map(|document| {
            let world = document.world();
            let world = world.borrow();
            (
                document.base_url(),
                world.origin.clone(),
                world.state.referrer_policy.clone(),
            )
        });
        let policy = self.inherited_policy(parent);
        let Some(document) = self.frames.get_mut(&child) else {
            return;
        };
        match inherited {
            Some((base, origin, referrer_policy)) => {
                document.load_frame_srcdoc(&base, origin, &policy, &referrer_policy, markup);
            }
            None => document.load_frame_srcdoc(
                &Url::parse("about:blank").expect("about:blank parses"),
                url::Origin::new_opaque(),
                &policy,
                "",
                markup,
            ),
        }
    }

    /// A `data:` frame URL decodes in-process. A malformed URL fails the
    /// navigation and leaves the frame on its current document
    /// (<https://fetch.spec.whatwg.org/#data-url-processor>).
    fn load_data_frame(&mut self, child: FrameId, parent: FrameId, url: &Url) {
        let policy = self.inherited_policy(parent);
        let Some((content_type, body)) = decode_data_url(url.as_str()) else {
            if let Some(document) = self.frames.get_mut(&child) {
                document.load_about_blank(None, None);
            }
            self.inherit_policy(child, parent);
            return;
        };
        if let Some(document) = self.frames.get_mut(&child) {
            // A `data:` URL inherits the initiator's policy container
            // (<https://html.spec.whatwg.org/multipage/browsers.html#determining-navigation-params-policy-container>).
            let mut head = ResponseHead::non_http(url.to_string(), Some(content_type));
            head.inherited_policy = Some(policy);
            document.install_frame_response(&head, &body, None);
        }
    }

    /// A `blob:` frame URL reads the parent's Blob URL Store. A missing entry
    /// is a network error. The child's origin is the entry's creator
    /// (<https://w3c.github.io/FileAPI/#blob-url-entry>).
    fn load_blob_frame(&mut self, child: FrameId, parent: FrameId, url: &Url) {
        let mut key = url.clone();
        key.set_fragment(None);
        let loaded = self.frames.get(&parent).and_then(|document| {
            let key = key.as_str();
            Some((
                document.object_url_contents(key)?,
                document.object_url_type(key),
                document.object_url_origin(key)?,
            ))
        });
        let Some((contents, content_type, origin)) = loaded else {
            return;
        };
        let policy = self.inherited_policy(parent);
        if let Some(document) = self.frames.get_mut(&child) {
            // A `blob:` URL inherits the initiator's policy container, and the
            // child's origin is the store entry's creator
            // (<https://html.spec.whatwg.org/multipage/browsers.html#determining-navigation-params-policy-container>).
            let mut head =
                ResponseHead::non_http(url.to_string(), content_type.as_deref().map(String::from));
            head.inherited_policy = Some(policy);
            document.install_frame_response(&head, contents.as_bytes(), Some(origin));
        }
    }

    /// Records a frame load so the parent's `load` event waits for it
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
    /// A later container navigation supersedes the insertion firing, so the
    /// recorded entry is cleared: the navigation's own `load` must fire.
    fn mark_frame_load_pending(&mut self, container: crate::dom::tree::NodeId, parent: FrameId) {
        if let Some(document) = self.frames.get_mut(&parent) {
            document.mark_frame_load_pending(container);
            let _ = document
                .world()
                .borrow_mut()
                .insertion_load_fired
                .remove(&container);
        }
    }

    fn apply_navigations(&mut self, navigations: Vec<(FrameId, FrameNavigation)>) {
        for (initiator, navigation) in navigations {
            let chosen = {
                let shared = self.runtime.shared.borrow();
                let tree = &shared.tree;
                match &navigation.target {
                    NavigationTarget::Container(container) => tree
                        .frame_for_container(*container)
                        .map(|frame| (frame, Some(*container))),
                    NavigationTarget::Target(target) => tree
                        .choose_navigable(target, initiator)
                        .map(|frame| (frame, tree.container(frame))),
                }
            };
            let Some((chosen, container)) = chosen else {
                continue;
            };
            self.navigate_frame(chosen, initiator, &navigation);
            if let Some(container) = container {
                self.publish_frame_document(container, chosen);
            }
        }
    }

    fn publish_streams(&mut self, streams: Vec<(FrameId, bool)>) {
        for (frame, closed) in streams {
            let container = self.runtime.shared.borrow().tree.container(frame);
            if let Some(container) = container {
                self.publish_frame_document(container, frame);
                if closed {
                    // The written document is a fresh load: it supersedes the
                    // insertion firing, so its own `load` must fire.
                    let parent = self.runtime.shared.borrow().tree.parent(frame);
                    if let Some(parent) = parent {
                        self.mark_frame_load_pending(container, parent);
                    }
                }
            }
        }
    }

    /// Routes finished sends to their target frame's task queue. The message
    /// queues on the document it was posted to, and the intended target origin
    /// is checked when the task runs
    /// ([window post message steps](https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps)).
    fn apply_deliveries(&mut self, deliveries: Vec<Delivery>) {
        for delivery in deliveries {
            match delivery {
                Delivery::WindowMessage {
                    target,
                    document,
                    source,
                    origin,
                    target_origin,
                    payload,
                    ports,
                } => {
                    // A task is not runnable once its document is not fully
                    // active
                    // (<https://html.spec.whatwg.org/multipage/webappapis.html#concept-task-runnable>).
                    match self.frames.get_mut(&target) {
                        Some(document_now) if document_now.matches_document(document) => {
                            Self::route_endpoints(&self.runtime.shared, &ports, target);
                            document_now.push_window_message(WindowMessage {
                                source,
                                origin,
                                target_origin,
                                payload,
                                ports,
                            });
                        }
                        _ => self.runtime.shared.borrow_mut().forget_endpoints(&ports),
                    }
                }
                Delivery::PortMessage {
                    endpoint,
                    payload,
                    ports,
                } => {
                    let owner = self.runtime.shared.borrow().ports.owner(endpoint);
                    match owner {
                        Some(frame) if self.frames.contains_key(&frame) => {
                            Self::route_endpoints(&self.runtime.shared, &ports, frame);
                            if let Some(document) = self.frames.get_mut(&frame) {
                                document.push_port_message(endpoint, payload, ports);
                            }
                        }
                        Some(_) => {
                            // The owning frame is gone; nothing can receive it.
                            self.runtime.shared.borrow_mut().forget_endpoints(&ports);
                        }
                        None => {
                            // The port is in transit; the received realm
                            // flushes its queue when the port is enabled
                            // (<https://html.spec.whatwg.org/multipage/web-messaging.html#transfer-receiving-steps>).
                            self.runtime
                                .shared
                                .borrow_mut()
                                .requeue_port_message(endpoint, payload, ports);
                        }
                    }
                }
                Delivery::PortClosed { endpoint } => {
                    let owner = self.runtime.shared.borrow().ports.owner(endpoint);
                    match owner {
                        Some(frame) if self.frames.contains_key(&frame) => {
                            if let Some(document) = self.frames.get_mut(&frame) {
                                document.push_port_closed(endpoint);
                            }
                        }
                        Some(_) => {}
                        None => self
                            .runtime
                            .shared
                            .borrow_mut()
                            .requeue_port_close(endpoint),
                    }
                }
            }
        }
    }

    /// Aims every transferred endpoint at `target`, whose realm materializes
    /// it when the carrying message is delivered
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#transfer-receiving-steps>).
    fn route_endpoints(shared: &SharedHandle, endpoints: &[u64], target: FrameId) {
        let mut shared = shared.borrow_mut();
        for endpoint in endpoints {
            shared.ports.route(*endpoint, target);
        }
    }

    /// Publishes `frame`'s document root as the active document of
    /// `container`, which is what `contentDocument` resolves through
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#dom-iframe-contentdocument>).
    fn publish_frame_document(&mut self, container: crate::dom::tree::NodeId, frame: FrameId) {
        let document = self.frames.get(&frame).and_then(Document::document_root);
        if let Some(document) = document {
            self.runtime
                .registry
                .borrow_mut()
                .set_frame_document(container, document);
        }
    }

    fn fire_ready_frame_loads(&mut self) -> bool {
        // Encoding-label maps append ~100 iframes; skip a realm for that
        // burst. A handful of navigated HTML frames still get a realm
        // before `load` so `contentDocument` wrappers have prototypes
        // (<https://html.spec.whatwg.org/multipage/window-object.html#the-window-object>).
        //
        // The count is global, not per parent: sharding the same iframes
        // across parents must not flip realm creation. Sorted for
        // determinism: without it the first-N cutoff depends on HashMap order
        // and scheduling, silently changing `instanceof` realms run to run.
        const EAGER_FRAME_REALM_CAP: usize = 8;
        let mut frame_ids: Vec<FrameId> = self.frames.keys().copied().collect();
        frame_ids.sort();
        let total_pending: usize = frame_ids
            .iter()
            .filter_map(|parent| self.frames.get(parent))
            .map(|document| document.pending_frame_loads().len())
            .sum();
        let eager_realm = total_pending <= EAGER_FRAME_REALM_CAP;
        let mut fired = false;
        for parent in frame_ids {
            let Some(document) = self.frames.get(&parent) else {
                continue;
            };
            let pending = document.pending_frame_loads();
            for container in pending {
                let Some(child) = self
                    .runtime
                    .shared
                    .borrow()
                    .tree
                    .frame_for_container(container)
                else {
                    if let Some(document) = self.frames.get_mut(&parent) {
                        document.cancel_frame_load(container);
                    }
                    continue;
                };
                let waiting = self
                    .frames
                    .get(&child)
                    .is_none_or(Document::waiting_for_load);
                if waiting {
                    continue;
                }
                // A navigated frame always gets its realm before `load`:
                // `contentWindow.location` must be readable in the load
                // handler, and the cap only exists to keep a burst of blank
                // parser-created iframes cheap. A blank frame has no URL of
                // its own to report.
                let navigated = self
                    .frames
                    .get(&child)
                    .is_some_and(|document| !document.is_initial_blank());
                if (eager_realm || navigated)
                    && let Some(document) = self.frames.get_mut(&child)
                {
                    document.ensure_js_ok();
                }
                self.publish_frame_document(container, child);
                if let Some(document) = self.frames.get_mut(&parent) {
                    fired |= document.finish_frame_load(container);
                }
            }
        }
        fired
    }

    pub fn shutdown(&mut self) {
        for document in self.frames.values_mut() {
            document.shutdown();
        }
    }

    pub fn release(&mut self) {
        for document in self.frames.values_mut() {
            document.release();
        }
    }
}

/// Decodes a `data:` URL into its MIME type and body bytes, or `None` when
/// the URL is malformed or its base64 payload is not decodable
/// (<https://fetch.spec.whatwg.org/#data-url-processor>).
///
/// An empty MIME defaults to `text/plain;charset=US-ASCII`, as the Fetch
/// processor requires; every success carries a MIME, so callers never see
/// `None` content types from this function.
#[must_use]
pub fn decode_data_url(raw: &str) -> Option<(String, Vec<u8>)> {
    let url = data_url::DataUrl::process(raw).ok()?;
    let (body, _fragment) = url.decode_to_vec().ok()?;
    Some((url.mime_type().to_string(), body))
}

/// Room for `data:` plus a MIME type before the comma.
///
/// A percent-encoded payload uses three characters per body byte (`%XX`).
/// The raw ceiling is three times [`crate::MAX_RESPONSE_BODY_BYTES`] plus this
/// allowance, so a body that fits the cap is not rejected for its encoding.
const DATA_URL_PREFIX_ALLOWANCE: usize = 1024;

/// Decodes a `data:` URL that fits the response body cap.
///
/// The raw-length check runs before decode. A `data:` URL is materialized
/// entirely in this process. Percent-encoding is the long form: one body
/// byte is `%XX`. Base64 is shorter (~4/3). The raw ceiling is three times
/// [`crate::MAX_RESPONSE_BODY_BYTES`] plus `DATA_URL_PREFIX_ALLOWANCE`. The
/// decoded body is then held to the cap. `None` means the raw URL is past
/// the ceiling, the URL is not decodable, or the body is past the cap.
///
/// <https://fetch.spec.whatwg.org/#data-url-processor>
#[must_use]
pub fn decode_data_url_bounded(raw: &str) -> Option<(String, Vec<u8>)> {
    let raw_ceiling = crate::protocol::MAX_RESPONSE_BODY_BYTES
        .saturating_mul(3)
        .saturating_add(DATA_URL_PREFIX_ALLOWANCE);
    if raw.len() > raw_ceiling {
        return None;
    }
    let (content_type, body) = decode_data_url(raw)?;
    if body.len() > crate::protocol::MAX_RESPONSE_BODY_BYTES {
        return None;
    }
    Some((content_type, body))
}

/// Decodes a URL into its UTF-8 text, percent-escapes included.
fn percent_decode(source: &str) -> String {
    percent_encoding::percent_decode_str(source)
        .decode_utf8_lossy()
        .into_owned()
}

/// Capture surface sides: at least 1 and at most the painter cap. A capture
/// is transient, so an out-of-range request is clamped rather than rejected.
fn capture_sides(width: u32, height: u32) -> Result<(u32, u32), TabError> {
    if width == 0 || height == 0 {
        return Err(TabError::Render {
            message: crate::render::RenderError::InvalidViewport.to_string(),
        });
    }
    Ok((
        width.min(crate::render::MAX_VIEWPORT_SIDE),
        height.min(crate::render::MAX_VIEWPORT_SIDE),
    ))
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Drop realms before the wrapper cache so cached JS values never
        // outlive the QuickJS heap. The engine's own runtime handle is the
        // last one and drops with the struct.
        self.frames.clear();
        self.runtime.registry.borrow_mut().clear();
        self.js_runtime.collect();
    }
}

#[cfg(test)]
mod data_url_tests {
    use super::decode_data_url_bounded;
    use crate::protocol::MAX_RESPONSE_BODY_BYTES;

    #[test]
    fn accepts_a_small_data_url() {
        let (content_type, body) = decode_data_url_bounded("data:text/plain,hi").expect("decode");
        assert_eq!(content_type, "text/plain");
        assert_eq!(body, b"hi");
    }

    #[test]
    fn rejects_a_malformed_data_url() {
        assert!(decode_data_url_bounded("data:text/plain;base64,!!!!").is_none());
    }

    #[test]
    fn rejects_a_raw_url_past_the_ceiling() {
        let raw_ceiling = MAX_RESPONSE_BODY_BYTES
            .saturating_mul(3)
            .saturating_add(super::DATA_URL_PREFIX_ALLOWANCE);
        let raw = format!("data:text/plain,{}", "x".repeat(raw_ceiling));
        assert!(raw.len() > raw_ceiling);
        assert!(decode_data_url_bounded(&raw).is_none());
    }

    #[test]
    fn rejects_a_decoded_body_past_the_cap() {
        let raw = format!(
            "data:text/plain,{}",
            "x".repeat(MAX_RESPONSE_BODY_BYTES.saturating_add(1))
        );
        assert!(decode_data_url_bounded(&raw).is_none());
    }

    fn percent_encoded(bytes: usize) -> String {
        let mut raw = String::with_capacity("data:text/plain,".len() + bytes.saturating_mul(3));
        raw.push_str("data:text/plain,");
        raw.push_str(&"%00".repeat(bytes));
        raw
    }

    #[test]
    fn accepts_a_percent_encoded_body_just_under_the_cap() {
        let raw = percent_encoded(MAX_RESPONSE_BODY_BYTES - 1);
        let (_content_type, body) = decode_data_url_bounded(&raw).expect("under the cap");
        assert_eq!(body.len(), MAX_RESPONSE_BODY_BYTES - 1);
    }

    #[test]
    fn rejects_a_percent_encoded_body_just_over_the_cap() {
        let raw = percent_encoded(MAX_RESPONSE_BODY_BYTES + 1);
        assert!(decode_data_url_bounded(&raw).is_none());
    }
}

#[cfg(test)]
mod messaging_tests {
    use super::{Engine, FrameId, Mount, ResponseHead};
    use crate::messaging::{Delivery, TargetOrigin};
    use crate::protocol::{
        BrowsingContextHost, DialCancellation, DialCompletion, DialFailure, DialRequest,
        MessagingHost, NetworkHost, StorageChange, StorageError, StorageHost, StorageKind,
    };
    use std::sync::Arc;
    use url::Url;

    struct NullServices;

    impl NetworkHost for NullServices {
        fn start_dial(
            &self,
            _request: DialRequest,
            completion: DialCompletion,
        ) -> DialCancellation {
            completion(Err(DialFailure::Connect));
            Box::new(|| {})
        }

        fn cookies_for(&self, _url: &Url) -> String {
            String::new()
        }

        fn set_cookie(&self, _value: &str, _url: &Url) {}
    }

    impl StorageHost for NullServices {
        fn storage_get(&self, _kind: StorageKind, _origin: &str, _key: &str) -> Option<String> {
            None
        }

        fn storage_keys(&self, _kind: StorageKind, _origin: &str) -> Vec<String> {
            Vec::new()
        }

        fn storage_set(
            &self,
            _kind: StorageKind,
            _origin: &str,
            _url: &str,
            _key: &str,
            _value: &str,
            _source: FrameId,
        ) -> Result<Option<StorageChange>, StorageError> {
            Ok(None)
        }

        fn storage_remove(
            &self,
            _kind: StorageKind,
            _origin: &str,
            _url: &str,
            _key: &str,
            _source: FrameId,
        ) -> Option<StorageChange> {
            None
        }

        fn storage_clear(
            &self,
            _kind: StorageKind,
            _origin: &str,
            _url: &str,
            _source: FrameId,
        ) -> Option<StorageChange> {
            None
        }
    }

    impl BrowsingContextHost for NullServices {
        fn window_open(&self, _url: &str, _name: &str, _features: &str) -> Option<u64> {
            None
        }

        fn window_close(&self, _tab: u64) {}

        fn window_opener(&self) -> Option<u64> {
            None
        }

        fn window_post_message(&self, _tab: u64, _origin: &str, _target: &str, _payload: &str) {}

        fn remote_session_get(&self, _tab: u64, _origin: &str, _key: &str) -> Option<String> {
            None
        }
    }

    impl MessagingHost for NullServices {
        fn broadcast_post(&self, _origin: &str, _name: &str, _payload: &str, _channel: u64) {}
    }

    fn engine() -> Engine {
        Engine::new(
            Arc::new(NullServices),
            Arc::new(crate::js::Stop::new()),
            Arc::new(tokio::sync::Notify::new()),
        )
    }

    fn mount(url: &str, body: &str) -> Mount {
        Mount {
            head: ResponseHead {
                url: url.to_owned(),
                ..ResponseHead::default()
            },
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn message_posted_before_navigation_never_fires_in_the_replacement_document() {
        let mut engine = engine();
        engine
            .mount_frame(
                FrameId::MAIN,
                &mount("https://example.test/one", "<!doctype html><p>one"),
            )
            .expect("mount");
        let _ready = engine.drain_ready();
        // The delivery names the document it was posted to: the target
        // window's document at post time
        // (<https://html.spec.whatwg.org/multipage/webappapis.html#queue-a-global-task>).
        let document = engine.frames[&FrameId::MAIN].id();
        engine
            .runtime
            .shared
            .borrow_mut()
            .deliveries
            .push(Delivery::WindowMessage {
                target: FrameId::MAIN,
                document,
                source: FrameId::MAIN,
                origin: "https://example.test".to_owned(),
                target_origin: TargetOrigin::Any,
                // `__tbEncode('queued', [], null).payload`.
                payload: r#"tb1:{"root":0,"nodes":[["string","queued"]]}"#.to_owned(),
                ports: Vec::new(),
            });
        // A navigation commits before the engine routes the delivery: the new
        // document listens for `message`, and the sender's message must not
        // reach it.
        engine
            .mount_frame(
                FrameId::MAIN,
                &mount(
                    "https://example.test/two",
                    "<!doctype html><script>window.__ready = true; window.__hits = 0; window.onmessage = function () { window.__hits++; };</script>",
                ),
            )
            .expect("replace");
        let _ready = engine.drain_ready();
        assert_eq!(
            engine
                .eval_in(FrameId::MAIN, "String(window.__hits)")
                .expect("hits"),
            "0",
            "the replacement document must not receive the old document's message"
        );
        assert_eq!(
            engine
                .eval_in(FrameId::MAIN, "String(window.__ready)")
                .expect("ready"),
            "true",
            "the new document must have run its script"
        );
    }

    #[test]
    fn remote_message_checks_the_target_origin_and_reports_the_sender_origin() {
        let mut engine = engine();
        engine
            .mount_frame(
                FrameId::MAIN,
                &mount(
                    "https://example.test/",
                    "<!doctype html><script>window.__hits = 0; window.__origin = ''; window.onmessage = function (event) { window.__hits++; window.__origin = event.origin; };</script>",
                ),
            )
            .expect("mount");
        let _ready = engine.drain_ready();
        let payload = r#"tb1:{"root":0,"nodes":[["string","cross-tab"]]}"#.to_owned();
        // A target origin that names a different document is checked when the
        // task runs, so the message never reaches this realm.
        engine.receive_remote_window_message(
            "https://other.test".to_owned(),
            "https://different.test",
            payload.clone(),
        );
        let _ready = engine.drain_ready();
        assert_eq!(
            engine
                .eval_in(FrameId::MAIN, "String(window.__hits)")
                .expect("hits"),
            "0",
            "a mismatched target origin must not deliver"
        );
        // The matching target origin delivers, and `event.origin` is the
        // sender's origin at post time, not an empty string.
        engine.receive_remote_window_message(
            "https://other.test".to_owned(),
            "https://example.test",
            payload,
        );
        let _ready = engine.drain_ready();
        assert_eq!(
            engine
                .eval_in(FrameId::MAIN, "String(window.__hits)")
                .expect("hits"),
            "1",
            "the matching target origin must deliver"
        );
        assert_eq!(
            engine
                .eval_in(FrameId::MAIN, "window.__origin")
                .expect("origin"),
            "https://other.test",
            "the delivered event carries the sender's origin"
        );
    }

    #[test]
    fn slash_target_origin_resolves_to_the_sender_origin_with_its_identity() {
        let mut engine = engine();
        // A `data:` document's origin is opaque, and `/` on an opaque sender
        // still delivers to the sender itself: same origin with itself.
        engine
            .mount_frame(
                FrameId::MAIN,
                &mount(
                    "data:text/html,opaque",
                    "<!doctype html><script>window.__hits = 0; window.onmessage = function () { window.__hits++; };</script>",
                ),
            )
            .expect("mount");
        engine
            .eval_in(FrameId::MAIN, "window.postMessage('self', '/');")
            .expect("post");
        let _ready = engine.drain_ready();
        assert_eq!(
            engine
                .eval_in(FrameId::MAIN, "String(window.__hits)")
                .expect("hits"),
            "1",
            "`/` must resolve to the sender's origin, and a document is same origin with itself"
        );
    }

    #[test]
    fn opaque_target_origins_never_match_each_other() {
        let sender = Url::parse("data:text/html,one").expect("sender").origin();
        let other = Url::parse("data:text/html,two").expect("other").origin();
        // Two opaque origins are never same origin, whatever they serialize to.
        assert!(!TargetOrigin::Exact(sender.clone()).admits(&other));
        assert!(TargetOrigin::Exact(sender.clone()).admits(&sender));
        assert!(TargetOrigin::Any.admits(&other));
    }
}
