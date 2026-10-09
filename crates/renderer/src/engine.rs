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
use crate::document::{Document, FrameRuntime, Stop, WindowMessage};

/// The engine's default frame viewport in device pixels, used until CDP
/// emulation (or a mount) sets another size.
pub const DEFAULT_VIEWPORT: (u32, u32) = (800, 600);

use crate::documents::DocumentStore;
use crate::js::{
    DocumentStreamCommand, FrameNavigation, NavigationTarget, RealmRegistry, SharedJsRuntime,
};
use crate::messaging::{Delivery, MAX_FRAMES, SharedHandle};
use crate::protocol::{BrowserServices, FrameId, Mount, RendererEvent, ResponseHead, TabError};
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

    fn create_frame(&mut self, parent: FrameId, container: crate::js::world::NodeId) -> FrameId {
        let frame = self.runtime.shared.borrow_mut().allocate_frame();
        let Some(parent_document) = self.frames.get(&parent) else {
            return frame;
        };
        let parent_url = parent_document.inherited_url();
        let parent_viewport = parent_document.viewport_size;
        let mut document = Document::with_shared(frame, &self.runtime, &self.init_scripts);
        // A child frame shares the tab's viewport until a content-box-driven
        // size exists; Blitz does not couple iframe layout to the child.
        document.set_viewport_size(parent_viewport);
        document.load_about_blank(Some(&parent_url));
        self.frames.insert(frame, document);
        self.runtime
            .shared
            .borrow_mut()
            .tree
            .add(parent, frame, container);
        frame
    }

    /// Adopts every child frame the realms created for themselves (an `iframe`
    /// insertion inside a script) and starts any `src` load it still needs.
    ///
    /// Returns whether any frame was adopted, so the reconcile loop runs again
    /// before the engine sleeps.
    fn adopt_pending_frames(&mut self) -> bool {
        for document in self.frames.values_mut() {
            document.adopt_pending_frames();
        }
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
                let src = self
                    .frames
                    .get(&parent)
                    .and_then(|document| document.frame_src(container));
                if let Some(src) = src {
                    self.navigate_frame(child, container, &src, "GET", &[], None);
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
                .clone_from(&mount.init_scripts);
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
        document.world().borrow_mut().history = head.history.clone();
        let url = Url::parse(&head.url).ok();
        document.begin_response(url.as_ref(), head.content_type.as_deref(), head.viewport);
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

    /// Runs every immediately ready task in every frame. Never blocks.
    pub fn drain_ready(&mut self) {
        // Reconcile around every task so a task that inserts an iframe is
        // visible to the next task: `contentDocument` is non-null from the
        // task after the connection, not inside the connecting task itself
        // (https://html.spec.whatwg.org/multipage/iframe-embed-object.html#dom-iframe-contentdocument).
        //
        // Frames take one task per pass, round-robin: messages posted to
        // different frames must run in send order, and draining one frame dry
        // would let a later message overtake an earlier one
        // (<https://html.spec.whatwg.org/multipage/webappapis.html#task-queue>).
        self.reconcile_frames();
        loop {
            self.drain_storage_events();
            let mut ran = false;
            let frames: Vec<FrameId> = self.frames.keys().copied().collect();
            for frame in frames {
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
    }

    /// Queues one remote `postMessage` payload on the main frame's task
    /// source; the frame dispatches a trusted `message` event with a null
    /// source.
    pub fn receive_remote_window_message(&mut self, payload: String) {
        if let Some(document) = self.frames.get_mut(&FrameId::MAIN) {
            document.push_remote_message(payload);
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
                let commands = document.take_document_stream();
                if !commands.is_empty() {
                    streams.push((frame, commands));
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
            self.apply_streams(streams);
            self.reorder_frames();
            self.apply_deliveries(deliveries);
            let fired_load = self.fire_ready_frame_loads();
            if !had_work && !fired_load {
                break;
            }
        }
    }

    /// Applies iframe connection transitions.
    fn apply_iframe_lifecycle(&mut self, events: Vec<(FrameId, crate::document::IframeLifecycle)>) {
        for (parent, event) in events {
            match event {
                crate::document::IframeLifecycle::Inserted(container) => {
                    if self
                        .runtime
                        .shared
                        .borrow()
                        .tree
                        .frame_for_container(container)
                        .is_some()
                    {
                        continue;
                    }
                    if self.runtime.shared.borrow().tree.len() >= MAX_FRAMES {
                        continue;
                    }
                    let child = self.create_frame(parent, container);
                    let src = self
                        .frames
                        .get(&parent)
                        .and_then(|document| document.frame_src(container));
                    self.navigate_frame(
                        child,
                        container,
                        src.as_deref().unwrap_or(""),
                        "GET",
                        &[],
                        None,
                    );
                    self.publish_frame_document(container, child);
                }
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
    fn remove_subtree(&mut self, container: crate::js::world::NodeId) {
        let Some(child) = self
            .runtime
            .shared
            .borrow()
            .tree
            .frame_for_container(container)
        else {
            return;
        };
        let descendants: Vec<crate::js::world::NodeId> = self
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
        self.runtime.shared.borrow_mut().tree.remove(child);
        // Dropping the document closes its ports and its realm.
        self.frames.remove(&child);
    }

    fn remove_descendants(&mut self, parent: FrameId) {
        let containers: Vec<crate::js::world::NodeId> = self
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

    /// Navigates a child frame to one URL, resolving it against the parent
    /// document and dispatching on its scheme
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    fn navigate_frame(
        &mut self,
        child: FrameId,
        container: crate::js::world::NodeId,
        spec: &str,
        method: &str,
        body: &[u8],
        content_type: Option<&str>,
    ) {
        let parent = self
            .runtime
            .shared
            .borrow()
            .tree
            .parent(child)
            .unwrap_or(child);
        let parent_url = self
            .frames
            .get(&parent)
            .map_or_else(|| String::from("about:blank"), Document::inherited_url);
        let resolved = (!spec.is_empty())
            .then(|| {
                self.frames
                    .get(&parent)
                    .and_then(|document| document.resolve_frame_url(spec))
            })
            .flatten();
        let Some(url) = resolved else {
            // About:blank, the initial document of every frame without a
            // usable `src`, inherits the parent's URL here so its origin does
            // (<https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>).
            // A frame the engine just created is already on that document;
            // reloading it would throw away its realm and anything a script
            // has already set on it.
            self.keep_initial_blank(child, &parent_url);
            self.mark_frame_load_pending(container, parent);
            return;
        };
        self.load_frame_url(child, url, parent, method, body, content_type);
        self.mark_frame_load_pending(container, parent);
    }

    /// Keeps a frame's initial `about:blank` document while it still has one,
    /// so a script-set realm survives; otherwise installs the inherited blank.
    fn keep_initial_blank(&mut self, child: FrameId, parent_url: &str) {
        if !self
            .frames
            .get(&child)
            .is_some_and(Document::is_initial_blank)
            && let Some(document) = self.frames.get_mut(&child)
        {
            document.load_about_blank(Some(parent_url));
        }
    }

    /// Starts the load one resolved frame URL names, dispatching on its scheme
    /// (<https://html.spec.whatwg.org/multipage/iframe-embed-object.html#process-the-iframe-attributes>).
    fn load_frame_url(
        &mut self,
        child: FrameId,
        url: Url,
        parent: FrameId,
        method: &str,
        body: &[u8],
        content_type: Option<&str>,
    ) {
        let parent_url = self
            .frames
            .get(&parent)
            .map_or_else(|| String::from("about:blank"), Document::inherited_url);
        match url.scheme() {
            "about" => self.keep_initial_blank(child, &parent_url),
            "data" => {
                if let Some((content_type, body)) = decode_data_url(url.as_str()) {
                    if let Some(document) = self.frames.get_mut(&child) {
                        document.load_frame_response(&url, content_type.as_deref(), &body);
                    }
                } else {
                    // A malformed data URL fails the navigation; the frame
                    // stays on its current document
                    // (<https://fetch.spec.whatwg.org/#data-url-processor>).
                    if let Some(document) = self.frames.get_mut(&child) {
                        document.load_about_blank(None);
                    }
                }
            }
            "javascript" => {
                let script = percent_decode(url.as_str());
                let script = script
                    .strip_prefix("javascript:")
                    .unwrap_or(&script)
                    .to_owned();
                let initial = self
                    .frames
                    .get(&child)
                    .is_some_and(Document::is_initial_blank);
                if let Some(document) = self.frames.get_mut(&child) {
                    if initial {
                        // The script runs in the document the browser already
                        // created; its result is discarded.
                        document.eval_frame_script(&script);
                    } else {
                        document.load_javascript_frame(Some(&parent_url), &script);
                    }
                }
            }
            "blob" => {
                let contents = self
                    .frames
                    .get(&parent)
                    .and_then(|document| document.object_url_contents(url.as_str()));
                let content_type = self
                    .frames
                    .get(&parent)
                    .and_then(|document| document.object_url_type(url.as_str()));
                if let Some(document) = self.frames.get_mut(&child) {
                    document.load_frame_response(
                        &url,
                        content_type.as_deref(),
                        contents.as_deref().unwrap_or("").as_bytes(),
                    );
                }
            }
            "http" | "https" => {
                let initiator = self
                    .frames
                    .get(&parent)
                    .and_then(|document| Url::parse(document.document_url()).ok())
                    .unwrap_or_else(|| url.clone());
                if let Some(document) = self.frames.get_mut(&child) {
                    document.navigate_to(
                        url,
                        initiator,
                        method.to_owned(),
                        body.to_vec(),
                        content_type.map(str::to_owned),
                    );
                }
            }
            _ => {
                if let Some(document) = self.frames.get_mut(&child) {
                    document.load_about_blank(None);
                }
            }
        }
    }

    /// Records a frame load so the parent's `load` event waits for it
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
    fn mark_frame_load_pending(&mut self, container: crate::js::world::NodeId, parent: FrameId) {
        if let Some(document) = self.frames.get_mut(&parent) {
            document.mark_frame_load_pending(container);
        }
    }

    fn apply_navigations(&mut self, navigations: Vec<(FrameId, FrameNavigation)>) {
        for (
            frame,
            FrameNavigation {
                target,
                spec,
                method,
                body,
                content_type,
            },
        ) in navigations
        {
            match target {
                NavigationTarget::Container(container) => {
                    let Some(child) = self
                        .runtime
                        .shared
                        .borrow()
                        .tree
                        .frame_for_container(container)
                    else {
                        continue;
                    };
                    self.navigate_frame(
                        child,
                        container,
                        &spec,
                        &method,
                        &body,
                        content_type.as_deref(),
                    );
                    self.publish_frame_document(container, child);
                }
                NavigationTarget::SelfFrame => {
                    let Ok(url) = Url::parse(&spec) else {
                        continue;
                    };
                    self.load_frame_url(frame, url, frame, &method, &body, content_type.as_deref());
                }
            }
        }
    }

    fn apply_streams(&mut self, streams: Vec<(FrameId, Vec<DocumentStreamCommand>)>) {
        for (frame, commands) in streams {
            let container = self.runtime.shared.borrow().tree.container(frame);
            let mut closed = false;
            if let Some(document) = self.frames.get_mut(&frame) {
                for command in commands {
                    closed |= matches!(&command, DocumentStreamCommand::Close);
                    document.apply_document_stream(command);
                }
            }
            if let Some(container) = container {
                self.publish_frame_document(container, frame);
                if closed
                    && let Some(parent) = self.runtime.shared.borrow().tree.parent(frame)
                    && let Some(document) = self.frames.get_mut(&parent)
                {
                    document.mark_frame_load_pending(container);
                }
            }
        }
    }

    /// Reorders each frame's children to the tree order of their containers.
    ///
    /// A same-document `iframe` move is not a connection transition
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-length>).
    fn reorder_frames(&mut self) {
        let parents: Vec<FrameId> = self
            .frames
            .keys()
            .copied()
            .filter(|frame| {
                !self
                    .runtime
                    .shared
                    .borrow()
                    .tree
                    .children(*frame)
                    .is_empty()
            })
            .collect();
        for parent in parents {
            let Some(document) = self.frames.get_mut(&parent) else {
                continue;
            };
            let containers = document.iframe_containers_in_order();
            self.runtime
                .shared
                .borrow_mut()
                .tree
                .reorder(parent, &containers);
        }
    }

    /// Routes finished sends to their target frame's task queue, re-checking
    /// the target origin at delivery time
    /// ([window post message steps](https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps)).
    fn apply_deliveries(&mut self, deliveries: Vec<Delivery>) {
        for delivery in deliveries {
            match delivery {
                Delivery::WindowMessage {
                    target,
                    source,
                    origin,
                    target_origin,
                    payload,
                    ports,
                } => {
                    let allowed = self.frames.get(&target).is_some_and(|document| {
                        target_origin == "*" || target_origin == document.origin_string()
                    });
                    // Blink re-checks the intended target origin against the
                    // recipient's origin at delivery time and drops the
                    // message on mismatch
                    // (LocalDOMWindow::DispatchMessageEventWithOriginCheck).
                    match (allowed, self.frames.get_mut(&target)) {
                        (true, Some(document)) => {
                            Self::route_endpoints(&self.runtime.shared, &ports, target);
                            document.push_window_message(WindowMessage {
                                source,
                                origin,
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
    fn publish_frame_document(&mut self, container: crate::js::world::NodeId, frame: FrameId) {
        let document = self.frames.get(&frame).and_then(Document::document_root);
        if let Some(document) = document {
            self.runtime
                .registry
                .borrow_mut()
                .set_frame_document(container, document);
        }
    }

    fn fire_ready_frame_loads(&mut self) -> bool {
        let mut fired = false;
        let frame_ids: Vec<FrameId> = self.frames.keys().copied().collect();
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
/// processor requires; callers treat the value as the response MIME.
pub(crate) fn decode_data_url(raw: &str) -> Option<(Option<String>, Vec<u8>)> {
    let url = data_url::DataUrl::process(raw).ok()?;
    let (body, _fragment) = url.decode_to_vec().ok()?;
    Some((Some(url.mime_type().to_string()), body))
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
