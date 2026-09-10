use std::collections::HashSet;
use std::future::{Future, pending, poll_fn};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use tokio::time::{Instant, sleep, sleep_until};

use super::{HostTimer, HtmlJob, MAX_QUEUED_JS_FETCHES, Page, PageEvent, QueuedDial};

impl Page {
    pub(crate) fn drive_for(&mut self, budget: Duration) {
        let _completed = self.run_until_timeout(budget, |_| false);
    }

    pub(crate) fn has_background_work(&self) -> bool {
        !self.jobs.is_empty()
            || !self.timers.is_empty()
            || self.in_flight_dials > 0
            || !self.queued_dials.is_empty()
            || self
                .js
                .as_ref()
                .is_some_and(crate::js::JsHost::has_pending_work)
    }

    /// HTML host timer: fire [`PageEvent::Timer`] after `delay`.
    #[must_use]
    pub fn schedule_timer(&mut self, delay: Duration) -> u32 {
        let id = self.next_timer_id;
        self.next_timer_id = self.next_timer_id.saturating_add(1);
        self.timers.push(HostTimer {
            id,
            when: Instant::now() + delay,
            fired: false,
        });
        id
    }

    /// Parks this thread as the page Tokio waiter until no jobs, timers,
    /// queued dials, or in-flight fetches remain. Must not run inside another
    /// runtime.
    ///
    /// # Panics
    ///
    /// If called from inside a Tokio runtime or the current-thread runtime
    /// cannot be built.
    pub fn run(&mut self) {
        self.block_on_pump(None, |page| !page.stopped());
    }

    /// Parks until the current navigation has fired `load`, without waiting
    /// for leftover host timers. `WebDriver` page-load strategy `normal`.
    ///
    /// [WebDriver navigate to URL](https://w3c.github.io/webdriver/#navigate-to)
    ///
    /// # Panics
    ///
    /// Same conditions as [`Page::run`].
    pub fn run_until_load(&mut self) {
        self.block_on_pump(None, |page| page.waiting_for_load() && !page.stopped());
    }

    /// Parks like [`Page::run_until_load`], returning `false` if `timeout` elapses first.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Page::run`].
    pub fn run_until_load_timeout(&mut self, timeout: Duration) -> bool {
        self.run_until_timeout(timeout, |page| !page.waiting_for_load())
    }

    /// Parks like [`Page::run`], but returns as soon as `stop` is true.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Page::run`].
    pub fn run_until(&mut self, mut stop: impl FnMut(&mut Self) -> bool) {
        self.block_on_pump(None, |page| !stop(page) && !page.stopped());
    }

    /// Parks like [`Page::run_until`], returning `false` if `timeout` elapses first.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Page::run`].
    pub fn run_until_timeout(
        &mut self,
        timeout: Duration,
        mut stop: impl FnMut(&mut Self) -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        let mut done = false;
        self.block_on_pump(Some(deadline), |page| {
            if Instant::now() >= deadline || page.stopped() {
                return false;
            }
            if stop(page) {
                done = true;
                return false;
            }
            true
        });
        done
    }

    fn block_on_pump(&mut self, cap: Option<Instant>, keep_waiting: impl FnMut(&mut Self) -> bool) {
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "Page::run must not run inside another Tokio runtime"
        );
        let runtime = self.runtime.take().unwrap_or_else(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("current-thread Tokio runtime for the page thread")
        });
        runtime.block_on(self.pump(cap, keep_waiting));
        self.runtime = Some(runtime);
    }

    pub(crate) fn shutdown_runtime(&mut self) {
        self.stop.request();
        self.queued_dials.clear();
        self.in_flight_dials = 0;
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }

    fn stopped(&self) -> bool {
        self.stop.is_set()
    }

    async fn pump(
        &mut self,
        cap: Option<Instant>,
        mut keep_waiting: impl FnMut(&mut Self) -> bool,
    ) {
        loop {
            while let Ok(completed) = self.dial_rx.try_recv() {
                self.in_flight_dials = self.in_flight_dials.saturating_sub(1);
                match completed {
                    Ok(done) => self.jobs.push_back(HtmlJob::DialFinished(done)),
                    Err(fail) => self.jobs.push_back(HtmlJob::DialFailed(fail)),
                }
            }
            self.adopt_js_work();
            self.launch_queued_dials();
            while let Some(job) = self.jobs.pop_front() {
                self.run_job(job);
                self.adopt_js_work();
                self.launch_queued_dials();
                if !keep_waiting(self) {
                    return;
                }
            }
            if !keep_waiting(self) {
                return;
            }
            if self
                .js
                .as_ref()
                .is_some_and(crate::js::JsHost::has_pending_work)
            {
                continue;
            }
            let fetches_pending = self.in_flight_dials > 0;
            let queued = !self.queued_dials.is_empty();
            let next_deadline = match (self.next_timer_deadline(), cap) {
                (Some(timer), Some(limit)) => Some(timer.min(limit)),
                (timer, limit) => timer.or(limit),
            };
            if !fetches_pending && !queued && next_deadline.is_none() {
                break;
            }
            if queued && !fetches_pending {
                sleep(Duration::from_millis(1)).await;
                continue;
            }
            let network_poll = fetches_pending.then(|| Instant::now() + Duration::from_millis(1));
            let wake_at = match (next_deadline, network_poll) {
                (Some(timer), Some(network)) => Some(timer.min(network)),
                (timer, network) => timer.or(network),
            };
            let deadline = wait_until(wake_at);
            let mut deadline = std::pin::pin!(deadline);
            let stop = Arc::clone(&self.stop);
            poll_fn(|cx| {
                stop.register(cx.waker());
                if stop.is_set() {
                    return Poll::Ready(());
                }
                if deadline.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(());
                }
                Poll::Pending
            })
            .await;
            while let Some(id) = self.due_timer() {
                self.jobs.push_back(HtmlJob::Timer(id));
            }
        }
    }

    pub(crate) fn waiting_for_load(&self) -> bool {
        self.queued_dials.iter().any(|dial| match dial {
            QueuedDial::Navigate { epoch, .. } => *epoch == self.nav_epoch,
            QueuedDial::ClassicScript { epoch, .. } => *epoch == self.js_epoch,
            QueuedDial::Fetch { .. } | QueuedDial::JsFetch { .. } => false,
        }) || self.nav_in_flight == Some(self.nav_epoch)
            || self.classic_fetch_in_flight
            || self.active_parser.is_some()
            || !self.world.borrow().document_ready
    }

    fn launch_queued_dials(&mut self) {
        let mut leftover = Vec::new();
        let queued = std::mem::take(&mut self.queued_dials);
        for dial in queued {
            let task_dial = dial.clone();
            let fetch = self.fetch.clone();
            let completed = self.dial_tx.clone();
            let stop = Arc::clone(&self.stop);
            if self
                .fetch
                .try_submit(move || {
                    if stop.is_set() {
                        return;
                    }
                    let result = super::navigate::send_dial(&fetch, &task_dial, &stop);
                    let _send_result = completed.send(result);
                })
                .is_err()
            {
                leftover.push(dial);
                continue;
            }
            if let QueuedDial::Navigate { epoch, .. } = &dial {
                self.nav_in_flight = Some(*epoch);
            }
            self.in_flight_dials = self.in_flight_dials.saturating_add(1);
        }
        leftover.extend(std::mem::take(&mut self.queued_dials));
        self.queued_dials = leftover;
    }

    fn run_job(&mut self, job: HtmlJob) {
        match job {
            HtmlJob::Timer(id) => {
                self.events.push(PageEvent::Timer(id));
                if let Some(js_id) = self.js_timer_slots.remove(&id)
                    && let Some(js) = &self.js
                {
                    super::note_script(&mut self.events, js.fire_timer(js_id).is_err());
                }
                self.timers.retain(|timer| timer.id != id);
                self.adopt_js_work();
            }
            HtmlJob::DialFinished(done) => self.finish_dial(done),
            HtmlJob::DialFailed(fail) => self.fail_dial(fail),
        }
    }

    pub(in crate::page) fn adopt_js_work(&mut self) {
        let timeouts = self
            .js
            .as_ref()
            .map(crate::js::JsHost::take_pending_timeouts)
            .unwrap_or_default();
        let fetches = self
            .js
            .as_ref()
            .map(crate::js::JsHost::take_pending_fetches)
            .unwrap_or_default();
        let cancels: HashSet<i32> = self
            .js
            .as_ref()
            .map(crate::js::JsHost::take_pending_cancels)
            .unwrap_or_default()
            .into_iter()
            .collect();
        for js_id in &cancels {
            if let Some(page_id) = self
                .js_timer_slots
                .iter()
                .find_map(|(&page_id, &id)| (id == *js_id).then_some(page_id))
            {
                self.js_timer_slots.remove(&page_id);
                self.timers.retain(|timer| timer.id != page_id);
            }
        }
        for timeout in timeouts {
            if cancels.contains(&timeout.js_id) {
                continue;
            }
            let id = self.schedule_timer(timeout.delay);
            self.js_timer_slots.insert(id, timeout.js_id);
        }
        for fetch in fetches {
            let queued_js_fetches = self
                .queued_dials
                .iter()
                .filter(|dial| matches!(dial, QueuedDial::JsFetch { .. }))
                .count();
            if queued_js_fetches >= MAX_QUEUED_JS_FETCHES {
                self.events.push(PageEvent::FetchFailed);
                self.settle_js_fetch(fetch.js_id, false, 0, "");
                continue;
            }
            if let Ok(url) = self.resolve_dial_url(&fetch.url) {
                let initiator = self.document_url.clone();
                self.queued_dials.push(QueuedDial::JsFetch {
                    url,
                    initiator,
                    id: fetch.js_id,
                    epoch: self.js_epoch,
                });
            } else {
                self.events.push(PageEvent::FetchFailed);
                self.settle_js_fetch(fetch.js_id, false, 0, "");
            }
        }
    }

    fn next_timer_deadline(&self) -> Option<Instant> {
        self.timers
            .iter()
            .filter(|timer| !timer.fired)
            .map(|timer| timer.when)
            .min()
    }

    fn due_timer(&mut self) -> Option<u32> {
        let now = Instant::now();
        let timer = self
            .timers
            .iter_mut()
            .filter(|timer| !timer.fired && timer.when <= now)
            .min_by_key(|timer| timer.when)?;
        timer.fired = true;
        Some(timer.id)
    }
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(when) => sleep_until(when).await,
        None => pending().await,
    }
}
