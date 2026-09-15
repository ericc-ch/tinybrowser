//! In-process page engine for hosts that do not spawn renderer processes.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::document::Stop;
use crate::engine::Engine;
use crate::{EngineHost, FrameId, Mount, RemoteValue, TabError, TabEvent};

/// One renderer hosted directly in its caller's process.
pub struct EmbeddedRenderer {
    engine: Engine,
    stop: Arc<Stop>,
}

impl EmbeddedRenderer {
    /// Creates a renderer whose external effects are handled by `host`.
    #[must_use]
    pub fn new(host: Arc<dyn EngineHost>) -> Self {
        let stop = Arc::new(Stop::new());
        let wake = Arc::new(Notify::new());
        Self {
            engine: Engine::new(host, Arc::clone(&stop), wake),
            stop,
        }
    }

    /// Replaces the main-frame document and runs every immediately ready task.
    ///
    /// # Errors
    ///
    /// The engine rejected the mount.
    pub fn mount(&mut self, mount: &Mount) -> Result<(), TabError> {
        self.engine.mount_frame(FrameId::MAIN, mount)?;
        self.engine.pump_ready();
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
            .execute_remote_in(FrameId::MAIN, source, timeout)
    }

    /// Runs every immediately ready page task without blocking.
    pub fn pump_ready(&mut self) {
        self.engine.pump_ready();
    }

    /// Removes and returns all pending main-frame events.
    ///
    /// # Errors
    ///
    /// The retained event limit was exceeded.
    pub fn take_events(&mut self) -> Result<Vec<TabEvent>, TabError> {
        self.engine
            .take_events()
            .map(|events| events.into_iter().map(|(_, event)| event).collect())
            .map_err(|()| TabError::RendererUnavailable {
                message: "embedded renderer event queue overflowed".into(),
            })
    }

    /// Duration until the next page timer is ready.
    #[must_use]
    pub fn time_until_deadline(&self) -> Option<Duration> {
        self.engine
            .next_deadline()
            .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
    }
}

impl Drop for EmbeddedRenderer {
    fn drop(&mut self) {
        self.stop.request();
        self.engine.shutdown();
    }
}
