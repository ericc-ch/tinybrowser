//! The page engine for one renderer process: the shared `QuickJS` heap, the Tokio
//! waiter, and the frame registry.
//!
//! [ADR 0014](../../../docs/adrs/0014-frames-and-per-frame-realms.md): one
//! `QuickJS` `Runtime` and one Tokio waiter per renderer process; `Document` is
//! one frame. Child frames join the same engine sharing both.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::document::{Document, Stop, Waiter};
use crate::documents::DocumentStore;
use crate::js::{DocumentStreamCommand, FrameNavigation, RealmRegistry, SharedJsRuntime};
use crate::protocol::{BrowserServices, FrameId, Mount, TabError, TabEvent};
use crate::{Parsed, RemoteValue, ScriptValue};

/// How long one frame may occupy the waiter before the engine gives the next
/// frame a turn.
const FRAME_STEP: Duration = Duration::from_millis(1);
const MAX_FRAMES: usize = 64;

/// One renderer process's page engine.
///
/// The main frame is the tab's top-level document; child frames share the
/// engine's heap and waiter, so same-site frames can pass JavaScript objects
/// synchronously.
pub struct Engine {
    js_runtime: SharedJsRuntime,
    waiter: Waiter,
    /// Every frame's trees, shared across their realms.
    documents: Rc<RefCell<DocumentStore>>,
    /// Document ownership and the shared wrapper cache.
    registry: Rc<RefCell<RealmRegistry>>,
    services: Arc<dyn BrowserServices>,
    stop: Arc<Stop>,
    frames: BTreeMap<FrameId, Document>,
    child_frames: HashMap<dom::NodeId, (FrameId, FrameId)>,
    pending_frame_loads: HashSet<dom::NodeId>,
    next_frame: u64,
}

impl Engine {
    /// An engine whose frames dial and read cookies through `services`.
    #[must_use]
    pub fn new(services: Arc<dyn BrowserServices>) -> Self {
        Self::with_stop(services, Arc::new(Stop::new()))
    }

    /// [`Engine::new`] with an externally owned stop flag, so an in-process
    /// host can interrupt a runaway script.
    pub fn with_stop(services: Arc<dyn BrowserServices>, stop: Arc<Stop>) -> Self {
        let js_runtime = SharedJsRuntime::default();
        let waiter = Waiter::new();
        let documents = Rc::new(RefCell::new(DocumentStore::default()));
        let registry = Rc::new(RefCell::new(RealmRegistry::default()));
        let main = Document::with_shared(
            Arc::clone(&services),
            js_runtime.clone(),
            waiter.clone(),
            Rc::clone(&documents),
            &registry,
            Arc::clone(&stop),
        );
        let mut frames = BTreeMap::new();
        frames.insert(FrameId::MAIN, main);
        Self {
            js_runtime,
            waiter,
            documents,
            registry,
            services,
            stop,
            frames,
            child_frames: HashMap::new(),
            pending_frame_loads: HashSet::new(),
            next_frame: 1,
        }
    }

    /// Creates a child frame on this engine's heap and waiter.
    pub fn create_frame(&mut self) -> FrameId {
        let frame = FrameId::new(self.next_frame);
        self.next_frame = self.next_frame.saturating_add(1);
        let document = Document::with_shared(
            Arc::clone(&self.services),
            self.js_runtime.clone(),
            self.waiter.clone(),
            Rc::clone(&self.documents),
            &self.registry,
            Arc::clone(&self.stop),
        );
        self.frames.insert(frame, document);
        frame
    }

    /// The frame with this id, when the engine still hosts it.
    #[must_use]
    pub fn frame_mut(&mut self, frame: FrameId) -> Option<&mut Document> {
        self.frames.get_mut(&frame)
    }

    /// Every frame the engine currently hosts, with its id.
    pub fn frames(&self) -> impl Iterator<Item = (FrameId, &Document)> {
        self.frames
            .iter()
            .map(|(&frame, document)| (frame, document))
    }

    /// The tab's main frame.
    fn main(&self) -> &Document {
        self.frames
            .get(&FrameId::MAIN)
            .expect("engine always hosts its main frame")
    }

    fn main_mut(&mut self) -> &mut Document {
        self.frames
            .get_mut(&FrameId::MAIN)
            .expect("engine always hosts its main frame")
    }

    /// Replaces the main frame's document from a host mount.
    pub fn mount(&mut self, mount: &Mount) {
        self.remove_descendants(FrameId::MAIN);
        self.main_mut().mount(mount);
        self.reconcile_frames();
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
        document.mount(mount);
        self.reconcile_frames();
        Ok(())
    }

    /// Parses `html` into the main frame and starts a new realm.
    pub fn load_html(&mut self, html: &str) {
        self.remove_descendants(FrameId::MAIN);
        self.main_mut().load_html(html);
        self.reconcile_frames();
    }

    /// Evaluates `source` in the main frame and returns its string coercion.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] when the engine cannot start or the script throws.
    pub fn eval(&mut self, source: &str) -> Result<String, TabError> {
        self.eval_in(FrameId::MAIN, source)
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

    /// Evaluates `source` in the main frame and returns a value-only result.
    ///
    /// # Errors
    ///
    /// Same as [`Engine::eval`].
    pub fn execute_script(&mut self, source: &str) -> Result<ScriptValue, TabError> {
        self.execute_script_in(FrameId::MAIN, source)
    }

    /// Evaluates `source` in one frame and returns a value-only result.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] or [`TabError::Script`].
    pub fn execute_script_in(
        &mut self,
        frame: FrameId,
        source: &str,
    ) -> Result<ScriptValue, TabError> {
        let result = self
            .frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .execute_script(source);
        self.reconcile_frames();
        result
    }

    /// Evaluates `source` and interns node handles for the protocol.
    ///
    /// # Errors
    ///
    /// Same as [`Engine::eval`].
    pub fn execute_remote(
        &mut self,
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<RemoteValue, TabError> {
        self.execute_remote_in(FrameId::MAIN, source, timeout)
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

    /// Sets the main frame's document URL.
    ///
    /// # Errors
    ///
    /// [`TabError::InvalidUrl`] when `url` is not an absolute URL.
    pub fn set_document_url(&mut self, url: &str) -> Result<(), TabError> {
        self.set_document_url_in(FrameId::MAIN, url)
    }

    /// Sets one frame's document URL.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] or [`TabError::InvalidUrl`].
    pub fn set_document_url_in(&mut self, frame: FrameId, url: &str) -> Result<(), TabError> {
        self.frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .set_document_url(url)
    }

    /// Main frame document URL.
    #[must_use]
    pub fn document_url(&self) -> &str {
        self.main().document_url()
    }

    /// Main frame document `Content-Language`, if any.
    #[must_use]
    pub fn content_language(&self) -> Option<&str> {
        self.main().content_language()
    }

    /// Runs `reader` against the main frame's active document, if any.
    pub fn with_parsed<R>(&self, reader: impl FnOnce(&Parsed) -> R) -> Option<R> {
        self.main().with_parsed(reader)
    }

    /// Jobs that have already run, in order, for every frame.
    #[must_use]
    pub fn events(&self) -> Vec<TabEvent> {
        self.frames
            .values()
            .flat_map(|document| document.events().iter().copied())
            .collect()
    }

    /// True when any frame has jobs, timers, dials, or pending JS work.
    #[must_use]
    pub fn has_background_work(&self) -> bool {
        !self.pending_frame_loads.is_empty()
            || self
                .frames
                .values()
                .any(|document| document.has_background_work() || document.has_engine_requests())
    }

    /// Advances every frame for at most `budget`.
    pub fn drive_for(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        loop {
            let frames: Vec<FrameId> = self.frames.keys().copied().collect();
            for frame in frames {
                let now = Instant::now();
                if now >= deadline {
                    return;
                }
                let Some(document) = self.frames.get_mut(&frame) else {
                    continue;
                };
                // Each frame gets at most one step, and never past the shared
                // deadline: many frames must not multiply the budget.
                document.drive_for(FRAME_STEP.min(deadline - now));
                self.reconcile_frames();
            }
            if Instant::now() >= deadline || !self.has_background_work() {
                return;
            }
        }
    }

    /// Parks the renderer thread until no frame has tasks, timers, queued
    /// dials, or fetches.
    pub fn run(&mut self) {
        while self.has_background_work() {
            let frames: Vec<FrameId> = self.frames.keys().copied().collect();
            for frame in frames {
                if let Some(document) = self.frames.get_mut(&frame) {
                    document.drive_for(FRAME_STEP);
                }
                self.reconcile_frames();
            }
        }
    }

    /// Parks until every frame has fired `load`.
    pub fn run_until_load(&mut self) {
        while self.frames.values().any(Document::waiting_for_load) {
            let frames: Vec<FrameId> = self.frames.keys().copied().collect();
            for frame in frames {
                if let Some(document) = self.frames.get_mut(&frame) {
                    document.drive_for(FRAME_STEP);
                }
                self.reconcile_frames();
            }
        }
    }

    fn reconcile_frames(&mut self) {
        loop {
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
            let had_work = !lifecycle.is_empty() || !navigations.is_empty() || !streams.is_empty();
            self.apply_lifecycle(lifecycle);
            self.apply_navigations(navigations);
            self.apply_streams(streams);
            let fired_load = self.fire_ready_frame_loads();
            if !had_work && !fired_load {
                break;
            }
        }
    }

    fn apply_lifecycle(&mut self, events: Vec<(FrameId, dom::Lifecycle)>) {
        for (parent, event) in events {
            match event {
                dom::Lifecycle::Inserted(container) => {
                    if self.child_frames.contains_key(&container) {
                        continue;
                    }
                    if self.frames.len() >= MAX_FRAMES {
                        continue;
                    }
                    let child = self.create_frame();
                    let parent_url = self
                        .frames
                        .get(&parent)
                        .map(|document| document.document_url().to_owned());
                    if let Some(document) = self.frames.get_mut(&child) {
                        if let Some(parent_url) = parent_url {
                            let _ = document.set_document_url(&parent_url);
                        }
                        document.load_html("");
                    }
                    self.child_frames.insert(container, (parent, child));
                    self.publish_frame_document(container, child);
                }
                dom::Lifecycle::Removed(container) => {
                    self.remove_subtree(container);
                }
            }
        }
    }

    /// Removes a frame and every descendant it owns before the backing
    /// document is replaced or disconnected. Lifecycle events can arrive only
    /// for the direct iframe, so recurse through the renderer's mirrored tree
    /// explicitly to avoid stale realms and pending loads.
    fn remove_subtree(&mut self, container: dom::NodeId) {
        let Some((_, child)) = self.child_frames.remove(&container) else {
            return;
        };
        let descendants: Vec<dom::NodeId> = self
            .child_frames
            .iter()
            .filter_map(|(&nested, &(parent, _))| (parent == child).then_some(nested))
            .collect();
        for nested in descendants {
            self.remove_subtree(nested);
        }
        self.pending_frame_loads.remove(&container);
        self.registry.borrow_mut().forget_frame(container);
        self.frames.remove(&child);
    }

    fn remove_descendants(&mut self, parent: FrameId) {
        let containers: Vec<dom::NodeId> = self
            .child_frames
            .iter()
            .filter_map(|(&container, &(owner, _))| (owner == parent).then_some(container))
            .collect();
        for container in containers {
            self.remove_subtree(container);
        }
    }

    fn apply_navigations(&mut self, navigations: Vec<FrameNavigation>) {
        for navigation in navigations {
            match navigation {
                FrameNavigation::ObjectUrl {
                    container,
                    contents,
                } => {
                    let Some((_, child)) = self.child_frames.get(&container).copied() else {
                        continue;
                    };
                    let parent_url = self
                        .child_frames
                        .get(&container)
                        .and_then(|(parent, _)| self.frames.get(parent))
                        .map(|document| document.document_url().to_owned());
                    if let Some(document) = self.frames.get_mut(&child) {
                        if let Some(parent_url) = parent_url {
                            let _ = document.set_document_url(&parent_url);
                        }
                        document.load_html(&contents);
                    }
                    self.publish_frame_document(container, child);
                    self.pending_frame_loads.insert(container);
                }
            }
        }
    }

    fn apply_streams(&mut self, streams: Vec<(FrameId, Vec<DocumentStreamCommand>)>) {
        for (frame, commands) in streams {
            let container = self
                .child_frames
                .iter()
                .find_map(|(&container, &(_, child))| (child == frame).then_some(container));
            let mut closed = false;
            if let Some(document) = self.frames.get_mut(&frame) {
                for command in commands {
                    closed |= matches!(&command, DocumentStreamCommand::Close);
                    document.apply_document_stream(command);
                }
            }
            if let Some(container) = container {
                self.publish_frame_document(container, frame);
                if closed {
                    self.pending_frame_loads.insert(container);
                }
            }
        }
    }

    fn publish_frame_document(&mut self, container: dom::NodeId, frame: FrameId) {
        let document = self.frames.get(&frame).and_then(Document::document_root);
        if let Some(document) = document {
            self.registry
                .borrow_mut()
                .set_frame_document(container, document);
        }
    }

    fn fire_ready_frame_loads(&mut self) -> bool {
        let ready: Vec<dom::NodeId> = self
            .pending_frame_loads
            .iter()
            .copied()
            .filter(|container| {
                self.child_frames
                    .get(container)
                    .and_then(|(_, child)| self.frames.get(child))
                    .is_some_and(|document| !document.waiting_for_load())
            })
            .collect();
        for container in &ready {
            self.pending_frame_loads.remove(container);
            let Some((parent, child)) = self.child_frames.get(container).copied() else {
                continue;
            };
            self.publish_frame_document(*container, child);
            if let Some(document) = self.frames.get_mut(&parent) {
                document.fire_node_load(*container);
            }
        }
        !ready.is_empty()
    }

    /// Stops every frame. Further work must go through [`Engine::new`].
    pub fn shutdown(&mut self) {
        for document in self.frames.values_mut() {
            document.shutdown();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Drop realms before the wrapper cache so cached JS values never
        // outlive the QuickJS heap. The engine's own runtime handle is the
        // last one and drops with the struct.
        self.frames.clear();
        self.registry.borrow_mut().clear();
    }
}
