//! Renderer process policy: soft budget, spare, and same-site reuse.
//!
//! Blank documents stay virtual, the manager keeps one unlocked spare, and
//! under the soft limit each live site instance receives its own renderer
//! process. Assignment release arrives as a notice from the registry's drop
//! path, so process shutdown is decided off the same state lock that reserves
//! assignments.

use std::collections::HashMap;
use std::io;
use std::sync::Weak;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError};

use tokio::sync::{Semaphore, mpsc};

use crate::assignment::{Assignment, AssignmentOptions, AssignmentRegistry};
use crate::browser::TabRegistry;
use crate::context::PartitionServices;
use crate::link::{RendererHandle, SpawnProcessOptions, spawn_process};
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
    tabs: TabRegistry,
    next: AtomicU64,
    slots: Arc<Semaphore>,
    registry: Arc<AssignmentRegistry>,
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
    /// Tab registry handed to every renderer service task.
    pub(crate) tabs: TabRegistry,
}

impl RendererProcessManager {
    pub(crate) fn new(
        browser: crate::browser::BrowserHandle,
        services: RendererProcessManagerOptions,
    ) -> Self {
        let RendererProcessManagerOptions {
            partition,
            sessions,
            tabs,
        } = services;
        let (releases, release_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(ManagerInner {
            partition,
            sessions,
            browser,
            tabs,
            next: AtomicU64::new(1),
            slots: Arc::new(Semaphore::new(renderer_process_limit())),
            registry: Arc::new(AssignmentRegistry::new(releases)),
            state: tokio::sync::Mutex::new(ManagerState::default()),
        });
        tokio::spawn(release_loop(Arc::downgrade(&inner), release_rx));
        fill_spare(&inner);
        Self { inner }
    }

    /// Creates one assignment for `options`, reusing a live same-site process
    /// when the budget requires it.
    ///
    /// # Errors
    ///
    /// Process spawn failure, a failed protocol handshake, or an exhausted
    /// process budget with no reusable process.
    pub(crate) async fn acquire(&self, options: AssignmentOptions) -> io::Result<Arc<Assignment>> {
        let (process, assignment) = self.acquire_reserved(options).await?;
        if let Err(error) = process.assign_notify(assignment.id()).await {
            // Dropping the assignment releases the reservation and asks the
            // manager to re-evaluate the process.
            drop(assignment);
            return Err(error);
        }
        fill_spare(&self.inner);
        Ok(assignment)
    }

    /// Picks a process and records the assignment while holding the state
    /// lock, so a concurrent release cannot observe zero assignments and shut
    /// the process down mid-acquire.
    async fn acquire_reserved(
        &self,
        options: AssignmentOptions,
    ) -> io::Result<(Arc<RendererHandle>, Arc<Assignment>)> {
        let mut state = self.inner.state.lock().await;
        let process = if let Some(spare) = state.spare.take() {
            spare.bind(&options.site)?;
            spare
        } else if let Some(process) = spawn_slot(&self.inner, Some(options.site.clone())).await? {
            process
        } else {
            let candidates = state.sites.entry(options.site.clone()).or_default();
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
        let processes = state.sites.entry(options.site.clone()).or_default();
        if !processes.iter().any(|candidate| {
            candidate
                .upgrade()
                .is_some_and(|candidate| Arc::ptr_eq(&candidate, &process))
        }) {
            processes.push(Arc::downgrade(&process));
        }
        let assignment = self.inner.registry.reserve(Arc::clone(&process), options);
        Ok((process, assignment))
    }
}

/// Evaluates process shutdown for every released assignment.
///
/// The notice carries the process, not the id: the registry already removed
/// the record and decremented the count before sending it. A release that
/// races an acquire either lands before the reservation (and finds a live
/// assignment) or after the process left the site pool, so the two paths
/// cannot disagree.
async fn release_loop(
    inner: Weak<ManagerInner>,
    mut releases: mpsc::UnboundedReceiver<Arc<RendererHandle>>,
) {
    while let Some(process) = releases.recv().await {
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let site = process
            .site
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut shutdown = false;
        {
            let mut state = inner.state.lock().await;
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
        fill_spare(&inner);
    }
}

fn fill_spare(inner: &Arc<ManagerInner>) {
    let inner = Arc::clone(inner);
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
            registry: Arc::clone(&inner.registry),
            tabs: inner.tabs.clone(),
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
