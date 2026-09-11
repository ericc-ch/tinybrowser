//! The page engine for one renderer process: the shared `QuickJS` heap, the Tokio
//! waiter, and the frame registry.
//!
//! [ADR 0014](../../../docs/adrs/0014-frames-and-per-frame-realms.md): one
//! `QuickJS` `Runtime` and one Tokio waiter per renderer process; `Document` is
//! one frame. Child frames join the same engine sharing both.

use std::cell::Ref;
use std::sync::Arc;
use std::time::Duration;

use crate::document::{Document, Stop, Waiter};
use crate::js::SharedJsRuntime;
use crate::protocol::{BrowserServices, Mount, TabError, TabEvent};
use crate::{Parsed, RemoteValue, ScriptValue};

/// One renderer process's page engine.
///
/// The first frame is the tab's main frame; the shared heap and waiter are
/// handed to every frame the engine creates, so same-site frames can pass
/// JavaScript objects synchronously.
pub struct Engine {
    main: Document,
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
        Self {
            main: Document::with_shared(services, js_runtime, waiter, stop),
        }
    }

    /// Replaces the main frame's document from a host mount.
    pub fn mount(&mut self, mount: &Mount) {
        self.main.mount(mount);
    }

    /// Parses `html` into the main frame and starts a new realm.
    pub fn load_html(&mut self, html: &str) {
        self.main.load_html(html);
    }

    /// Evaluates `source` in the main frame and returns its string coercion.
    ///
    /// # Errors
    ///
    /// [`TabError::Script`] when the engine cannot start or the script throws.
    pub fn eval(&mut self, source: &str) -> Result<String, TabError> {
        self.main.eval(source)
    }

    /// Evaluates `source` in the main frame and returns a value-only result.
    ///
    /// # Errors
    ///
    /// Same as [`Engine::eval`].
    pub fn execute_script(&mut self, source: &str) -> Result<ScriptValue, TabError> {
        self.main.execute_script(source)
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
        self.main.execute_remote(source, timeout)
    }

    /// Sets the main frame's document URL.
    ///
    /// # Errors
    ///
    /// [`TabError::InvalidUrl`] when `url` is not an absolute URL.
    pub fn set_document_url(&mut self, url: &str) -> Result<(), TabError> {
        self.main.set_document_url(url)
    }

    /// Main frame document URL.
    #[must_use]
    pub fn document_url(&self) -> &str {
        self.main.document_url()
    }

    /// Main frame document `Content-Language`, if any.
    #[must_use]
    pub fn content_language(&self) -> Option<&str> {
        self.main.content_language()
    }

    /// Main frame last parse result, if any.
    #[must_use]
    pub fn parsed(&self) -> Option<Ref<'_, Parsed>> {
        self.main.parsed()
    }

    /// Jobs that have already run, in order.
    #[must_use]
    pub fn events(&self) -> &[TabEvent] {
        self.main.events()
    }

    /// True when the engine has no jobs, timers, dials, or pending JS work.
    #[must_use]
    pub fn has_background_work(&self) -> bool {
        self.main.has_background_work()
    }

    /// Advances every frame for at most `budget`.
    pub fn drive_for(&mut self, budget: Duration) {
        self.main.drive_for(budget);
    }

    /// Parks the renderer thread until no frame has tasks, timers, queued
    /// dials, or fetches.
    pub fn run(&mut self) {
        self.main.run();
    }

    /// Parks until the main frame has fired `load`.
    pub fn run_until_load(&mut self) {
        self.main.run_until_load();
    }

    /// Stops every frame. Further work must go through [`Engine::new`].
    pub fn shutdown(&mut self) {
        self.main.shutdown();
    }
}
