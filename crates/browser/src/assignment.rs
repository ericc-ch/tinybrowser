//! Assignment registry: the browser-owned record of every live document.
//!
//! One [`Assignment`] per top-level document a renderer hosts. The registry
//! resolves renderer calls to their authority record, routes document events
//! to subscribers, and owns release. Dropping the last handle removes the
//! record, tells the renderer to release the engine, and asks the process
//! manager to evaluate shutdown. RAII release is the point: an aborted or
//! panicked tab coordinator cannot strand an engine or poison a shared
//! renderer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tokio::sync::mpsc;
use url::{Origin, Url};

use crate::actor::TabId;
use crate::link::{
    HandleMountOptions, HandleStartResponseOptions, RendererHandle, ResponseWriter,
};
use crate::network::TabNetworkHandle;
use crate::site::Site;
use crate::wire::{Command as RendererCommand, RendererAssignmentId, Reply};
use renderer::{FrameId, Mount, RendererEvent, TabError};

/// Bounded event handoff to one assignment's subscribers. Saturation is a
/// protocol violation: dropping lifecycle events would corrupt tab state,
/// while blocking the reader could strand a reply behind those events.
const EVENT_SUBSCRIBER_CAPACITY: usize = 4096;

/// Browser-owned inputs for creating one assignment.
pub(crate) struct AssignmentOptions {
    /// Tab that owns the document.
    pub(crate) tab: TabId,
    /// Site lock the hosting process was bound to.
    pub(crate) site: Site,
    /// Network handle for the tab's dials and cookies.
    pub(crate) network: TabNetworkHandle,
}

/// One browser-authorized top-level document inside a renderer process.
pub(crate) struct Assignment {
    id: RendererAssignmentId,
    process: Arc<RendererHandle>,
    registry: Arc<AssignmentRegistry>,
    /// Tab that owns the document.
    pub(crate) tab: TabId,
    /// Site lock the hosting process was bound to.
    pub(crate) site: Site,
    /// Network handle for the tab's dials and cookies.
    pub(crate) network: TabNetworkHandle,
    /// Origin of the top-level document the browser last mounted, recorded by
    /// the browser, never by the renderer. `None` before the first mount;
    /// opaque for `about:blank` and friends.
    committed_origin: Mutex<Option<Origin>>,
    subscribers: Mutex<Vec<mpsc::Sender<(FrameId, RendererEvent)>>>,
}

/// One document mount into the assignment's renderer.
pub(crate) struct AssignmentMountOptions {
    /// Target frame.
    pub(crate) frame: FrameId,
    /// HTTP status of the mounted document.
    pub(crate) status: u16,
    /// Completed document to mount.
    pub(crate) mount: Mount,
}

/// One streamed response start into the assignment's renderer.
pub(crate) struct AssignmentStartResponseOptions<'a> {
    /// Target frame.
    pub(crate) frame: FrameId,
    /// HTTP status of the mounted document.
    pub(crate) status: u16,
    /// Response metadata; the body streams separately.
    pub(crate) mount: &'a Mount,
}

impl Assignment {
    /// The renderer-minted identity of this document.
    #[must_use]
    pub(crate) fn id(&self) -> RendererAssignmentId {
        self.id
    }

    /// Records the origin the browser mounted for this top-level document.
    pub(crate) fn set_committed_origin(&self, origin: Origin) {
        *self
            .committed_origin
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(origin);
    }

    /// The browser-owned committed origin of this top-level document.
    #[must_use]
    pub(crate) fn committed_origin(&self) -> Option<Origin> {
        self.committed_origin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Authorizes a renderer-supplied origin string for a top-level claim: a
    /// committed document must match its origin exactly; opaque documents
    /// fall back to the process site lock.
    #[must_use]
    pub(crate) fn authorize_origin(&self, spec: &str) -> bool {
        let Ok(url) = Url::parse(spec) else {
            return false;
        };
        match self.committed_origin() {
            Some(origin) if origin.is_tuple() => url.origin() == origin,
            _ => self.site.authorize(spec).is_some(),
        }
    }

    pub(crate) async fn request(&self, command: RendererCommand) -> Result<Reply, TabError> {
        self.process.request(self.id, command).await
    }

    /// Streams one request whose reply is bytes (screenshots).
    pub(crate) async fn request_bytes(
        &self,
        command: RendererCommand,
    ) -> Result<Vec<u8>, TabError> {
        self.process.request_bytes(self.id, command).await
    }

    pub(crate) async fn mount(&self, options: AssignmentMountOptions) -> Result<Reply, TabError> {
        let AssignmentMountOptions {
            frame,
            status,
            mount,
        } = options;
        self.process
            .mount(HandleMountOptions {
                assignment: self.id,
                frame,
                status,
                mount,
            })
            .await
    }

    pub(crate) async fn start_response(
        &self,
        options: AssignmentStartResponseOptions<'_>,
    ) -> Result<ResponseWriter, TabError> {
        let AssignmentStartResponseOptions {
            frame,
            status,
            mount,
        } = options;
        self.process
            .start_response(HandleStartResponseOptions {
                assignment: self.id,
                frame,
                status,
                mount,
            })
            .await
    }

    /// Subscribes to this document's frame-tagged events after this call.
    #[must_use]
    pub(crate) fn subscribe(&self) -> mpsc::Receiver<(FrameId, RendererEvent)> {
        let (tx, rx) = mpsc::channel(EVENT_SUBSCRIBER_CAPACITY);
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// Delivers one event to every subscriber; returns whether a subscriber
    /// was too slow to accept it (the caller treats saturation as a protocol
    /// violation).
    pub(crate) fn publish_event(&self, frame: FrameId, event: RendererEvent) -> bool {
        let mut saturated = false;
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|subscriber| match subscriber.try_send((frame, event)) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    saturated = true;
                    false
                }
            });
        saturated
    }
}

impl Drop for Assignment {
    fn drop(&mut self) {
        // Note the release before the record disappears so a renderer call
        // that races this drop is answered as "released", never as a
        // violation. The renderer notice is best-effort: a dead renderer
        // needs none.
        logging::debug!(
            target: "browser::assignment",
            "assignment released: tab={:?} id={:?}",
            self.tab,
            self.id
        );
        self.registry.note_released(self.id);
        self.registry
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
        self.process.try_notify_release(self.id);
        self.process.assignments.fetch_sub(1, Ordering::Relaxed);
        // The process manager evaluates shutdown off this notice; a send can
        // only fail once the manager is gone, and then there is nothing left
        // to shut down.
        let _ = self.registry.releases.send(Arc::clone(&self.process));
    }
}

/// Every live assignment of one browser context, keyed by renderer identity.
///
/// The registry is shared by the process manager (creation, shutdown policy)
/// and by each renderer's service task (call resolution, event routing).
pub(crate) struct AssignmentRegistry {
    live: Mutex<HashMap<RendererAssignmentId, Weak<Assignment>>>,
    next: AtomicU64,
    /// Highest released assignment id. Ids increase monotonically, so a call
    /// for an id at or below the mark names an assignment this registry
    /// released; a call for a higher id was never assigned here.
    max_released: AtomicU64,
    releases: mpsc::UnboundedSender<Arc<RendererHandle>>,
}

impl AssignmentRegistry {
    pub(crate) fn new(releases: mpsc::UnboundedSender<Arc<RendererHandle>>) -> Self {
        Self {
            live: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            max_released: AtomicU64::new(0),
            releases,
        }
    }

    /// Records one assignment and counts it against its process.
    ///
    /// The caller holds the process manager's state lock, so a concurrent
    /// release cannot observe zero assignments and shut the process down
    /// while this one is being created.
    pub(crate) fn reserve(
        self: &Arc<Self>,
        process: Arc<RendererHandle>,
        options: AssignmentOptions,
    ) -> Arc<Assignment> {
        let id = RendererAssignmentId::new(self.next.fetch_add(1, Ordering::Relaxed));
        let assignment = Arc::new(Assignment {
            id,
            process,
            registry: Arc::clone(self),
            tab: options.tab,
            site: options.site,
            network: options.network,
            committed_origin: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
        });
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::downgrade(&assignment));
        assignment.process.assignments.fetch_add(1, Ordering::Relaxed);
        assignment
    }

    /// The live assignment named by `id`, if any.
    pub(crate) fn resolve(&self, id: RendererAssignmentId) -> Option<Arc<Assignment>> {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        let weak = live.get(&id)?;
        if let Some(assignment) = weak.upgrade() {
            return Some(assignment);
        }
        live.remove(&id);
        drop(live);
        self.note_released(id);
        None
    }

    /// Whether `id` names an assignment this registry already released.
    /// Assignment ids start at 1, so a call for 0 was never assigned here.
    pub(crate) fn was_released(&self, id: RendererAssignmentId) -> bool {
        id.get() != 0 && id.get() <= self.max_released.load(Ordering::Acquire)
    }

    fn note_released(&self, id: RendererAssignmentId) {
        self.max_released.fetch_max(id.get(), Ordering::AcqRel);
    }
}
