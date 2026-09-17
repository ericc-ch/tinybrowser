//! The page engine for one renderer process: the shared `QuickJS` heap, the Tokio
//! waiter, and the frame registry.
//!
//! One `QuickJS` `Runtime` and one Tokio waiter per renderer process;
//! `Document` is one frame. Child frames join the same engine sharing both,
//! while each keeps its own realm: JS values never cross realms, so the
//! engine routes serialized payloads and owns the channel endpoints
//! ([`crate::messaging`]).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;
use url::Url;

use crate::RemoteValue;
use crate::document::{Document, FrameRuntime, Stop, WindowMessage};
use crate::documents::DocumentStore;
use crate::js::{DocumentStreamCommand, FrameNavigation, RealmRegistry, SharedJsRuntime};
use crate::messaging::{Delivery, MAX_FRAMES};
use crate::protocol::{BrowserServices, FrameId, Mount, TabError, TabEvent};

/// One renderer process's page engine.
///
/// The main frame is the tab's top-level document; child frames share the
/// engine's heap and wake handle, so same-site frames can pass JavaScript
/// objects synchronously.
pub struct Engine {
    /// The handles every frame document shares.
    runtime: FrameRuntime,
    frames: BTreeMap<FrameId, Document>,
}

impl Engine {
    /// Builds an engine whose effects go through `services`.
    ///
    /// `stop` cancels every frame at once; `wake` is notified when a dial
    /// completion or stop arrives, so a carrier can wait instead of polling.
    #[must_use]
    pub fn new(services: Arc<dyn BrowserServices>, stop: Arc<Stop>, wake: Arc<Notify>) -> Self {
        let runtime = FrameRuntime {
            services,
            js_runtime: SharedJsRuntime::default(),
            wake,
            stop,
            documents: Rc::new(RefCell::new(DocumentStore::default())),
            registry: Rc::new(RefCell::new(RealmRegistry::default())),
            shared: Rc::new(RefCell::new(crate::messaging::Shared::default())),
        };
        let main = Document::with_shared(FrameId::MAIN, &runtime);
        let mut frames = BTreeMap::new();
        frames.insert(FrameId::MAIN, main);
        Self { runtime, frames }
    }

    fn create_frame(&mut self, parent: FrameId, container: dom::NodeId) -> FrameId {
        let frame = self.runtime.shared.borrow_mut().allocate_frame();
        let parent_url = self
            .frames
            .get(&parent)
            .map_or_else(|| String::from("about:blank"), Document::inherited_url);
        let Some(parent_document) = self.frames.get(&parent) else {
            return frame;
        };
        let mut document = parent_document
            .world()
            .borrow()
            .create_frame_document(frame);
        document.load_about_blank(Some(&parent_url));
        self.frames.insert(frame, document);
        self.runtime.shared.borrow_mut().tree.add(parent, frame, container);
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
        let mut batch = Vec::new();
        loop {
            for (&parent, document) in &mut self.frames {
                for (child, container, document) in document.take_new_frames() {
                    batch.push((parent, child, container, document));
                }
            }
            if batch.is_empty() {
                return false;
            }
            for (parent, child, container, document) in batch.drain(..) {
                self.frames.insert(child, document);
                let src = self
                    .frames
                    .get(&parent)
                    .and_then(|document| document.frame_src(container));
                if let Some(src) = src {
                    self.navigate_frame(child, container, &src);
                }
                if let Some(document) = self.frames.get_mut(&parent) {
                    document.mark_frame_load_pending(container);
                    document.flush_frame_proxy_sets(child);
                }
                self.publish_frame_document(container, child);
            }
        }
    }

    fn frame_mut(&mut self, frame: FrameId) -> Option<&mut Document> {
        self.frames.get_mut(&frame)
    }

    /// Replaces one frame's document from a host mount.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host `frame`.
    pub fn mount_frame(&mut self, frame: FrameId, mount: &Mount) -> Result<(), TabError> {
        self.remove_descendants(frame);
        let document = self
            .frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?;
        document.mount(mount)?;
        self.reconcile_frames();
        Ok(())
    }

    /// Opens a response body for `frame`, whose bytes arrive through
    /// [`Engine::write_body`] until [`Engine::end_body`].
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host the frame.
    pub fn open_body(
        &mut self,
        frame: FrameId,
        url: Option<&Url>,
        content_type: Option<&str>,
        content_language: Option<&str>,
    ) -> Result<(), TabError> {
        self.remove_descendants(frame);
        let document = self
            .frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?;
        document.begin_response(url, content_type, content_language);
        Ok(())
    }

    /// Feeds more response bytes to a frame opened by [`Engine::open_body`].
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host the frame.
    pub fn write_body(&mut self, frame: FrameId, bytes: &[u8]) -> Result<(), TabError> {
        self.frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .write_body(bytes);
        self.reconcile_frames();
        Ok(())
    }

    /// Ends a response body opened by [`Engine::open_body`].
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host the frame.
    pub fn end_body(&mut self, frame: FrameId) -> Result<(), TabError> {
        self.frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .end_body();
        self.reconcile_frames();
        Ok(())
    }

    /// Abandons a response body that will not be finished.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host the frame.
    pub fn abort_body(&mut self, frame: FrameId) -> Result<(), TabError> {
        self.frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .abort_body();
        self.reconcile_frames();
        Ok(())
    }

    /// Evaluates `source` in one frame and returns its string coercion.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] or [`TabError::Script`].
    pub fn eval_in(&mut self, frame: FrameId, source: &str) -> Result<String, TabError> {
        let result = self
            .frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .eval(source);
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
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<RemoteValue, TabError> {
        let result = self
            .frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .execute_remote(source, timeout);
        self.reconcile_frames();
        result
    }

    /// Removes and returns every pending event, with its frame.
    ///
    /// # Errors
    ///
    /// [`TabError::RendererUnavailable`] when a frame retained more events
    /// than its limit; the queue empties and the excess is lost.
    pub fn take_events(&mut self) -> Result<Vec<(FrameId, TabEvent)>, TabError> {
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
                self.runtime.registry.borrow_mut().forget_frame_world(*frame);
                self.frames.remove(frame);
            }
            let frame_ids: Vec<FrameId> = self.frames.keys().copied().collect();
            let mut lifecycle = Vec::new();
            let mut navigations = Vec::new();
            let mut streams = Vec::new();
            for frame in frame_ids {
                let Some(document) = self.frames.get_mut(&frame) else {
                    continue;
                };
                lifecycle.extend(
                    document
                        .take_lifecycle()
                        .into_iter()
                        .map(|event| (frame, event)),
                );
                navigations.extend(document.take_frame_navigations());
                let commands = document.take_document_stream();
                if !commands.is_empty() {
                    streams.push((frame, commands));
                }
            }
            let deliveries = self.runtime.shared.borrow_mut().take_deliveries();
            let had_work = adopted
                || !lifecycle.is_empty()
                || !navigations.is_empty()
                || !streams.is_empty()
                || !deliveries.is_empty();
            let reordered = self.apply_lifecycle(lifecycle);
            self.apply_navigations(navigations);
            self.apply_streams(streams);
            self.reorder_frames(&reordered);
            self.apply_deliveries(deliveries);
            let fired_load = self.fire_ready_frame_loads();
            if !had_work && !fired_load {
                break;
            }
        }
    }

    /// Applies connection transitions and returns the parents whose child
    /// order may have changed.
    fn apply_lifecycle(&mut self, events: Vec<(FrameId, dom::Lifecycle)>) -> Vec<FrameId> {
        let mut reordered = Vec::new();
        for (parent, event) in events {
            match event {
                dom::Lifecycle::Inserted(container) => {
                    if self.runtime.shared.borrow().tree.frame_for_container(container).is_some() {
                        continue;
                    }
                    if self.frames.len() >= MAX_FRAMES {
                        continue;
                    }
                    let child = self.create_frame(parent, container);
                    let src = self
                        .frames
                        .get(&parent)
                        .and_then(|document| document.frame_src(container));
                    self.navigate_frame(child, container, src.as_deref().unwrap_or(""));
                    self.publish_frame_document(container, child);
                    reordered.push(parent);
                }
                dom::Lifecycle::Removed(container) => {
                    self.remove_subtree(container);
                    reordered.push(parent);
                }
            }
        }
        reordered
    }

    /// Removes a frame and every descendant it owns before the backing
    /// document is replaced or disconnected. Lifecycle events can arrive only
    /// for the direct iframe, so recurse through the frame tree explicitly to
    /// avoid stale realms and pending loads.
    fn remove_subtree(&mut self, container: dom::NodeId) {
        let Some(child) = self
            .runtime.shared
            .borrow()
            .tree
            .frame_for_container(container)
        else {
            return;
        };
        let descendants: Vec<dom::NodeId> = self
            .runtime.shared
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
        let containers: Vec<dom::NodeId> = self
            .runtime.shared
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
    fn navigate_frame(&mut self, child: FrameId, container: dom::NodeId, spec: &str) {
        let parent = self.runtime.shared.borrow().tree.parent(child).unwrap_or(child);
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
            if !self.is_initial_blank(child, &parent_url)
                && let Some(document) = self.frames.get_mut(&child)
            {
                document.load_about_blank(Some(&parent_url));
            }
            if let Some(document) = self.frames.get_mut(&parent) {
                document.mark_frame_load_pending(container);
            }
            return;
        };
        match url.scheme() {
            "about" => {
                if !self.is_initial_blank(child, &parent_url)
                    && let Some(document) = self.frames.get_mut(&child)
                {
                    document.load_about_blank(Some(&parent_url));
                }
            }
            "data" => {
                let (content_type, body) = decode_data_url(url.as_str());
                if let Some(document) = self.frames.get_mut(&child) {
                    document.load_frame_response(
                        &url,
                        content_type.as_deref(),
                        None,
                        &body,
                    );
                }
            }
            "javascript" => {
                let script = percent_decode(url.as_str());
                let script = script
                    .strip_prefix("javascript:")
                    .unwrap_or(&script)
                    .to_owned();
                let initial = self.is_initial_blank(child, &parent_url);
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
                        None,
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
                    document.navigate_to(url, initiator);
                }
            }
            _ => {
                if let Some(document) = self.frames.get_mut(&child) {
                    document.load_about_blank(None);
                }
            }
        }
        if let Some(document) = self.frames.get_mut(&parent) {
            document.mark_frame_load_pending(container);
        }
    }

    fn apply_navigations(&mut self, navigations: Vec<FrameNavigation>) {
        for navigation in navigations {
            match navigation {
                FrameNavigation::Src { container, spec } => {
                    let Some(child) = self
                        .runtime.shared
                        .borrow()
                        .tree
                        .frame_for_container(container)
                    else {
                        continue;
                    };
                    self.navigate_frame(child, container, &spec);
                    self.publish_frame_document(container, child);
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

    /// Reorders each parent's children to the tree order of their containers.
    fn reorder_frames(&mut self, parents: &[FrameId]) {
        let mut seen = HashSet::new();
        for &parent in parents {
            if !seen.insert(parent) {
                continue;
            }
            let Some(document) = self.frames.get(&parent) else {
                continue;
            };
            let containers = document.iframe_containers_in_order();
            self.runtime.shared.borrow_mut().tree.reorder(parent, &containers);
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
                        None => self.runtime.shared.borrow_mut().requeue_port_close(endpoint),
                    }
                }
            }
        }
    }

    /// Whether `frame` is still on the `about:blank` document that inherits
    /// `parent_url`, which the browser creates at iframe insertion.
    fn is_initial_blank(&self, frame: FrameId, parent_url: &str) -> bool {
        self.frames
            .get(&frame)
            .is_some_and(|document| document.document_url() == parent_url)
    }

    fn publish_frame_document(&mut self, container: dom::NodeId, frame: FrameId) {
        let document = self.frames.get(&frame).and_then(Document::document_root);
        if let Some(document) = document {
            self.runtime.registry
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
                let Some(child) = self.runtime.shared.borrow().tree.frame_for_container(container) else {
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

/// Decodes a `data:` URL into its content type and body bytes
/// (<https://fetch.spec.whatwg.org/#data-url-processor>).
fn decode_data_url(raw: &str) -> (Option<String>, Vec<u8>) {
    let Some(rest) = raw.strip_prefix("data:") else {
        return (None, Vec::new());
    };
    let Some((metadata, body)) = rest.split_once(',') else {
        return (None, Vec::new());
    };
    let (content_type, is_base64) = match metadata.strip_suffix(";base64") {
        Some(metadata) => (metadata, true),
        None => (metadata, false),
    };
    let decoded = if is_base64 {
        decode_base64(body)
    } else {
        percent_decode_bytes(body)
    };
    let content_type = (!content_type.is_empty()).then(|| content_type.to_owned());
    (content_type, decoded)
}

/// Percent-decodes an ASCII URL component; indices outside `%XX` are kept.
fn percent_decode_bytes(source: &str) -> Vec<u8> {
    let bytes = source.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2]))
        {
            output.push((high << 4) | low);
            index += 3;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    output
}

/// Decodes a URL into its UTF-8 text, percent-escapes included.
fn percent_decode(source: &str) -> String {
    String::from_utf8_lossy(&percent_decode_bytes(source)).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decodes standard base64, ignoring ASCII whitespace and padding errors.
fn decode_base64(source: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [0xff_u8; 256];
    for (value, byte) in TABLE.iter().enumerate() {
        lookup[*byte as usize] = u8::try_from(value).unwrap_or(0);
    }
    let mut output = Vec::with_capacity(source.len() / 4 * 3);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for byte in source.bytes() {
        let digit = lookup[byte as usize];
        if digit == 0xff {
            if byte == b'=' {
                break;
            }
            continue;
        }
        buffer = (buffer << 6) | u32::from(digit);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    output
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Drop realms before the wrapper cache so cached JS values never
        // outlive the QuickJS heap. The engine's own runtime handle is the
        // last one and drops with the struct.
        self.frames.clear();
        self.runtime.registry.borrow_mut().clear();
    }
}
