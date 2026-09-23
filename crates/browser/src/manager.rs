//! Renderer process policy: soft budget, spare, and same-site reuse.
//!
//! Blank documents stay virtual, the manager keeps one unlocked spare, and
//! under the soft limit each live site instance receives its own renderer
//! process.

use std::collections::HashMap;
use std::io;
use std::sync::Weak;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError};

use crate::wire::RendererAssignmentId;
use tokio::sync::Semaphore;

use crate::context::PartitionServices;
use crate::link::{
    AssignmentContext, RendererAssignment, RendererHandle, SpawnProcessOptions, spawn_process,
};
use crate::site::Site;
use crate::storage::SessionStorage;

/// Identity of one live renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RendererId(pub(crate) u64);

/// Browser-owned renderer process manager.
pub(crate) struct RendererProcessManager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    partition: PartitionServices,
    sessions: Arc<SessionStorage>,
    browser: crate::browser::BrowserHandle,
    next: AtomicU64,
    next_assignment: AtomicU64,
    slots: Arc<Semaphore>,
    state: tokio::sync::Mutex<ManagerState>,
}

#[derive(Default)]
struct ManagerState {
    spare: Option<Arc<RendererHandle>>,
    sites: HashMap<Site, Vec<Weak<RendererHandle>>>,
    spawning_spare: bool,
}

/// Browser-owned dependencies one renderer manager runs with.
pub(crate) struct RendererProcessManagerOptions {
    /// Services shared by renderer assignments in the partition.
    pub(crate) partition: PartitionServices,
    /// Session storage owned by the browser.
    pub(crate) sessions: Arc<SessionStorage>,
}

impl RendererProcessManager {
    pub(crate) fn new(
        browser: crate::browser::BrowserHandle,
        services: RendererProcessManagerOptions,
    ) -> Self {
        let RendererProcessManagerOptions {
            partition,
            sessions,
        } = services;
        let manager = Self {
            inner: Arc::new(ManagerInner {
                partition,
                sessions,
                browser,
                next: AtomicU64::new(1),
                next_assignment: AtomicU64::new(1),
                slots: Arc::new(Semaphore::new(renderer_process_limit())),
                state: tokio::sync::Mutex::new(ManagerState::default()),
            }),
        };
        manager.fill_spare();
        manager
    }

    /// Creates a renderer locked to `site`.
    ///
    /// # Errors
    ///
    /// Process spawn failure or a failed protocol handshake.
    pub(crate) async fn acquire(
        &self,
        context: AssignmentContext,
    ) -> io::Result<Arc<RendererAssignment>> {
        let process = self.acquire_process(&context.site).await?;
        let assignment =
            RendererAssignmentId::new(self.inner.next_assignment.fetch_add(1, Ordering::Relaxed));
        if let Err(error) = process.assign(assignment, context).await {
            process.unreserve_assignment();
            return Err(error);
        }
        self.fill_spare();
        Ok(Arc::new(RendererAssignment {
            id: assignment,
            process,
        }))
    }

    async fn acquire_process(&self, site: &Site) -> io::Result<Arc<RendererHandle>> {
        let mut state = self.inner.state.lock().await;
        let process = if let Some(spare) = state.spare.take() {
            spare.bind(site)?;
            spare
        } else if let Some(process) = spawn_slot(&self.inner, Some(site.clone())).await? {
            process
        } else {
            let candidates = state.sites.entry(site.clone()).or_default();
            candidates.retain(|candidate| {
                candidate
                    .upgrade()
                    .is_some_and(|process| process.alive.load(Ordering::Relaxed))
            });
            candidates
                .iter()
                .find_map(Weak::upgrade)
                .ok_or_else(|| io::Error::other("renderer process budget exhausted"))?
        };
        let processes = state.sites.entry(site.clone()).or_default();
        if !processes.iter().any(|candidate| {
            candidate
                .upgrade()
                .is_some_and(|candidate| Arc::ptr_eq(&candidate, &process))
        }) {
            processes.push(Arc::downgrade(&process));
        }
        // Reserve the assignment before the state lock is released so a
        // concurrent release cannot observe zero and shut this process down.
        process.reserve_assignment();
        Ok(process)
    }

    pub(crate) async fn release(&self, assignment: Arc<RendererAssignment>) {
        let process = Arc::clone(&assignment.process);
        process.release(assignment.id).await;
        drop(assignment);
        // Decide shutdown under the same lock that `acquire_process` reserves
        // assignments with, so a new acquisition either keeps the process or
        // arrives after it is removed from the site pool.
        let site = process
            .site
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut shutdown = false;
        {
            let mut state = self.inner.state.lock().await;
            if process.assignments.load(Ordering::Relaxed) == 0 {
                if let Some(site) = &site
                    && let Some(processes) = state.sites.get_mut(site)
                {
                    processes.retain(|candidate| {
                        candidate
                            .upgrade()
                            .is_some_and(|candidate| !Arc::ptr_eq(&candidate, &process))
                    });
                    if processes.is_empty() {
                        state.sites.remove(site);
                    }
                }
                shutdown = true;
            }
        }
        if shutdown {
            process.shutdown().await;
        }
        drop(process);
        self.fill_spare();
    }

    fn fill_spare(&self) {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            {
                let mut state = inner.state.lock().await;
                if state.spare.is_some() || state.spawning_spare {
                    return;
                }
                state.spawning_spare = true;
            }
            let result = match spawn_slot(&inner, None).await {
                Ok(Some(spare)) => Ok(spare),
                Ok(None) => Err(io::Error::other("renderer process budget exhausted")),
                Err(error) => Err(error),
            };
            let mut state = inner.state.lock().await;
            state.spawning_spare = false;
            if let Ok(spare) = result {
                state.spare = Some(spare);
            }
        });
    }
}

/// Takes one slot from the process budget and spawns a renderer into it.
///
/// `Ok(None)` means the budget is exhausted; the caller decides whether to
/// reuse a live process or fail.
async fn spawn_slot(
    inner: &Arc<ManagerInner>,
    site: Option<Site>,
) -> io::Result<Option<Arc<RendererHandle>>> {
    let Ok(slot) = Arc::clone(&inner.slots).try_acquire_owned() else {
        return Ok(None);
    };
    let id = RendererId(inner.next.fetch_add(1, Ordering::Relaxed));
    Ok(Some(Arc::new(
        spawn_process(SpawnProcessOptions {
            id,
            site,
            partition: inner.partition.clone(),
            sessions: Arc::clone(&inner.sessions),
            browser: inner.browser.clone(),
            slot,
        })
        .await?,
    )))
}

fn renderer_process_limit() -> usize {
    if let Some(limit) = std::env::var_os("TINYBROWSER_RENDERER_PROCESS_LIMIT")
        .and_then(|value| value.to_str()?.parse::<usize>().ok())
        .filter(|limit| *limit > 0)
    {
        return limit;
    }
    let available_kib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("MemAvailable:")?
                    .split_whitespace()
                    .next()?
                    .parse::<usize>()
                    .ok()
            })
        })
        .unwrap_or(512 * 1024);
    (available_kib / (64 * 1024)).clamp(1, 128)
}
