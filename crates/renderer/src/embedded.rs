//! In-process page engine for hosts that do not spawn renderer processes.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::engine::Engine;
use crate::js::Stop;
use crate::{BrowserServices, FrameId, Mount, RemoteValue, RendererEvent, TabError};

/// One renderer hosted directly in its caller's process.
pub struct EmbeddedRenderer {
    engine: Engine,
    stop: Arc<Stop>,
    /// Ready tasks left queued by the last budgeted drain.
    ///
    /// Those tasks are often timers already marked fired, so
    /// [`Engine::next_deadline`] is `None` while work remains.
    ready_queued: bool,
}

impl EmbeddedRenderer {
    /// Creates a renderer whose external effects are handled by `host`.
    #[must_use]
    pub fn new(host: Arc<dyn BrowserServices>) -> Self {
        let stop = Arc::new(Stop::new());
        Self {
            // An in-process embedder with no browser around it gets the
            // built-in identity; a carrier that has one passes its own.
            engine: Engine::new(
                host,
                Arc::clone(&stop),
                Arc::new(Notify::new()),
                net::Identity::default(),
            ),
            stop,
            ready_queued: false,
        }
    }

    /// Replaces the main-frame document and runs ready tasks until the
    /// script budget.
    ///
    /// A yield leaves tasks queued. [`Self::drain_ready`] reports that so
    /// the host can step again immediately: timers already marked fired are
    /// not a future deadline.
    ///
    /// # Errors
    ///
    /// The engine rejected the mount.
    pub fn mount(&mut self, mount: &Mount) -> Result<(), TabError> {
        self.engine.mount_frame(FrameId::MAIN, mount)?;
        self.drain_until(std::time::Instant::now() + crate::TURN_SCRIPT_BUDGET);
        Ok(())
    }

    /// Evaluates JavaScript in the main frame and returns its string coercion.
    ///
    /// # Errors
    ///
    /// Script evaluation failed.
    pub fn eval(&mut self, source: &str) -> Result<String, TabError> {
        self.engine.eval_in(FrameId::MAIN, source)
    }

    /// Evaluates JavaScript in the main frame and returns a value-only result.
    ///
    /// # Errors
    ///
    /// Script evaluation failed.
    pub fn execute(
        &mut self,
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<RemoteValue, TabError> {
        self.engine
            .execute_remote_in(FrameId::MAIN, crate::ScriptSource::Page(source), timeout)
    }

    /// Runs ready tasks until the script budget without blocking.
    ///
    /// Returns whether ready work stayed queued. The host steps again
    /// immediately in that case.
    #[must_use]
    pub fn drain_ready(&mut self) -> bool {
        self.drain_until(std::time::Instant::now() + crate::TURN_SCRIPT_BUDGET)
    }

    /// Drains until `deadline`. Returns whether ready work stayed queued.
    ///
    /// [`Engine::drain_ready_until`] returns true only after `deadline` has
    /// passed, so one call spends the budget.
    fn drain_until(&mut self, deadline: std::time::Instant) -> bool {
        let yielded = self.engine.drain_ready_until(deadline);
        self.ready_queued = yielded;
        yielded
    }

    /// Removes and returns all pending main-frame events.
    ///
    /// # Errors
    ///
    /// The retained event limit was exceeded.
    pub fn take_events(&mut self) -> Result<Vec<(FrameId, RendererEvent)>, TabError> {
        self.engine.take_events()
    }

    /// Duration until the next page timer is ready.
    ///
    /// `Some(0)` when the last drain left ready work queued. Fired timers
    /// are not a deadline, and a host that waited on `None` would never
    /// run them.
    #[must_use]
    pub fn time_until_deadline(&self) -> Option<Duration> {
        if self.ready_queued {
            return Some(Duration::ZERO);
        }
        self.engine
            .next_deadline()
            .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
    }

    /// Mounts without draining, so a test can bound the following drain.
    #[cfg(test)]
    pub(crate) fn mount_for_test(&mut self, mount: &Mount) -> Result<(), TabError> {
        self.engine.mount_frame(FrameId::MAIN, mount)
    }

    /// [`Self::drain_ready`] bounded by `budget` from now.
    #[cfg(test)]
    pub(crate) fn drain_for(&mut self, budget: Duration) -> bool {
        self.drain_until(std::time::Instant::now() + budget)
    }
}

impl Drop for EmbeddedRenderer {
    fn drop(&mut self) {
        self.stop.request();
        self.engine.shutdown();
    }
}
