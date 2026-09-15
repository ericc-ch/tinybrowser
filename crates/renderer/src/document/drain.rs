use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use super::{Document, MAX_PENDING_JS_FETCHES, QueuedDial, Task, Timer};
use crate::protocol::TabEvent;

impl Document {
    /// Runs every immediately ready page task. Never blocks.
    ///
    /// The renderer loop owns all waiting: it sleeps until [`Document::next_deadline`]
    /// or until a dial completion, channel message, or stop wakes it, then calls
    /// this method again. That is why the page engine needs no private runtime.
    pub(crate) fn drain_ready(&mut self) {
        loop {
            self.adopt_dial_completions();
            while let Some(id) = self.due_timer() {
                self.tasks.push_back(Task::Timer(id));
            }
            self.adopt_js_work();
            self.launch_queued_dials();
            self.adopt_dial_completions();
            if let Some(task) = self.tasks.pop_front() {
                self.run_task(task);
                if self.stopped() {
                    return;
                }
                continue;
            }
            if self.stopped() {
                return;
            }
            if self
                .js
                .as_ref()
                .is_some_and(crate::js::JsRealm::has_pending_work)
            {
                continue;
            }
            return;
        }
    }

    fn adopt_dial_completions(&mut self) {
        while let Ok(completed) = self.dial_rx.try_recv() {
            self.in_flight_dials = self.in_flight_dials.saturating_sub(1);
            match completed {
                Ok(done) => self.tasks.push_back(Task::DialFinished(done)),
                Err(fail) => self.tasks.push_back(Task::DialFailed(fail)),
            }
        }
    }

    /// Earliest timer deadline this document waits for.
    #[must_use]
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.next_timer_deadline()
    }

    fn schedule_timer(&mut self, delay: Duration) -> u32 {
        let id = self.next_timer_id;
        self.next_timer_id = self.next_timer_id.saturating_add(1);
        self.timers.push(Timer {
            id,
            when: Instant::now() + delay,
            fired: false,
        });
        id
    }

    /// Stops the frame and drops its queued work. The shared `QuickJS` heap
    /// belongs to the page engine, not the frame, so it is not touched.
    pub(crate) fn shutdown(&mut self) {
        self.stop.request();
        self.release();
    }

    pub(crate) fn release(&mut self) {
        self.queued_dials.clear();
        self.in_flight_dials = 0;
        self.decoder = None;
        self.world.borrow_mut().forget_owned_documents();
    }

    fn stopped(&self) -> bool {
        self.stop.is_set()
    }

    pub(crate) fn waiting_for_load(&self) -> bool {
        self.queued_dials.iter().any(|dial| match dial {
            QueuedDial::ClassicScript { epoch, .. } => *epoch == self.js_epoch,
            QueuedDial::JsFetch { .. } => false,
        }) || self.classic_fetch_in_flight
            || self.active_parser.is_some()
            || !self.world.borrow().document_ready
    }

    fn launch_queued_dials(&mut self) {
        let queued = std::mem::take(&mut self.queued_dials);
        for dial in queued {
            let task_dial = dial.clone();
            let completed = self.dial_tx.clone();
            let wake = Arc::clone(&self.wake);
            let stop = Arc::clone(&self.stop);
            let request = super::dial::request(&dial);
            self.in_flight_dials = self.in_flight_dials.saturating_add(1);
            self.services.start_dial(
                request,
                Box::new(move |outcome| {
                    if stop.is_set() {
                        return;
                    }
                    let result = super::dial::complete(&task_dial, outcome);
                    let _send_result = completed.send(result);
                    wake.notify_one();
                }),
            );
        }
    }

    fn run_task(&mut self, task: Task) {
        match task {
            Task::Timer(id) => {
                self.record_event(TabEvent::Timer(id));
                if let Some(js_id) = self.js_timer_slots.remove(&id)
                    && let Some(js) = &self.js
                    && js.fire_timer(js_id).is_err()
                {
                    self.record_event(TabEvent::ScriptFailed);
                }
                self.timers.retain(|timer| timer.id != id);
                self.adopt_js_work();
            }
            Task::DialFinished(done) => self.finish_dial(done),
            Task::DialFailed(fail) => self.fail_dial(fail),
        }
    }

    pub(in crate::document) fn adopt_js_work(&mut self) {
        let timeouts = self
            .js
            .as_ref()
            .map(crate::js::JsRealm::take_pending_timeouts)
            .unwrap_or_default();
        let fetches = self
            .js
            .as_ref()
            .map(crate::js::JsRealm::take_pending_fetches)
            .unwrap_or_default();
        let cancels: HashSet<i32> = self
            .js
            .as_ref()
            .map(crate::js::JsRealm::take_pending_cancels)
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
            let pending_js_fetches = self.in_flight_dials.saturating_add(
                self.queued_dials
                    .iter()
                    .filter(|dial| matches!(dial, QueuedDial::JsFetch { .. }))
                    .count(),
            );
            if pending_js_fetches >= MAX_PENDING_JS_FETCHES {
                self.record_event(TabEvent::FetchFailed);
                self.settle_js_fetch(fetch.js_id, false, 0, "");
                continue;
            }
            if let Ok(url) = self.resolve_dial_url(&fetch.url) {
                let initiator = self.url.clone();
                self.queued_dials.push(QueuedDial::JsFetch {
                    url,
                    initiator,
                    id: fetch.js_id,
                    epoch: self.js_epoch,
                });
            } else {
                self.record_event(TabEvent::FetchFailed);
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
