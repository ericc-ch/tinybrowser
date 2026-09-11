//! The page engine for one renderer process: the shared `QuickJS` heap, the Tokio
//! waiter, and the frame registry.
//!
//! [ADR 0014](../../../docs/adrs/0014-frames-and-per-frame-realms.md): one
//! `QuickJS` `Runtime` and one Tokio waiter per renderer process; `Document` is
//! one frame. Child frames join the same engine sharing both.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::document::{Document, Stop, Waiter};
use crate::documents::DocumentStore;
use crate::js::{RealmRegistry, SharedJsRuntime};
use crate::protocol::{BrowserServices, FrameId, Mount, TabError, TabEvent};
use crate::{Parsed, RemoteValue, ScriptValue};

/// How long one frame may occupy the waiter before the engine gives the next
/// frame a turn.
const FRAME_STEP: Duration = Duration::from_millis(1);

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
        self.main_mut().mount(mount);
    }

    /// Replaces one frame's document from a host mount.
    ///
    /// # Errors
    ///
    /// [`TabError::UnknownFrame`] when the engine does not host `frame`.
    pub fn mount_frame(&mut self, frame: FrameId, mount: &Mount) -> Result<(), TabError> {
        let document = self
            .frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?;
        document.mount(mount);
        Ok(())
    }

    /// Parses `html` into the main frame and starts a new realm.
    pub fn load_html(&mut self, html: &str) {
        self.main_mut().load_html(html);
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
        self.frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .eval(source)
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
        self.frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .execute_script(source)
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
        self.frame_mut(frame)
            .ok_or(TabError::UnknownFrame { frame: frame.get() })?
            .execute_remote(source, timeout)
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
        self.frames.values().any(Document::has_background_work)
    }

    /// Advances every frame for at most `budget`.
    pub fn drive_for(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        loop {
            for document in self.frames.values_mut() {
                document.drive_for(FRAME_STEP);
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
            for document in self.frames.values_mut() {
                document.drive_for(FRAME_STEP);
            }
        }
    }

    /// Parks until every frame has fired `load`.
    pub fn run_until_load(&mut self) {
        while self.frames.values().any(Document::waiting_for_load) {
            for document in self.frames.values_mut() {
                document.drive_for(FRAME_STEP);
            }
        }
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
