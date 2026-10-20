use std::collections::HashSet;
use std::time::Duration;

use tokio::time::Instant;

use super::{DialContext, Document, MAX_PENDING_JS_FETCHES, QueuedDial, Task, Timer};
use crate::protocol::RendererEvent;

impl Document {
    /// Runs one batch of ready work and reports whether more may follow.
    ///
    /// The engine calls this instead of draining the whole document at once so
    /// it can reconcile frame documents between tasks: a task that inserts an
    /// iframe must be visible to the next task's `contentDocument` read.
    pub(crate) fn drain_step(&mut self) -> bool {
        self.adopt_dial_completions();
        while let Some(id) = self.due_timer() {
            self.tasks.push_back(Task::Timer(id));
        }
        self.adopt_js_work();
        self.drain_blitz_fetches();
        self.launch_queued_dials();
        self.adopt_dial_completions();
        if let Some(task) = self.tasks.pop_front() {
            // Joins the drain's turn when one is open, so serial tasks share
            // its deadline. A task outside a drain still opens its own turn.
            let _turn = self.js.as_ref().map(crate::js::JsRealm::enter_turn);
            self.run_task(task);
            return !self.stop.is_set();
        }
        false
    }

    /// Whether a task is queued or a timer is already due.
    pub(crate) fn has_ready_work(&self) -> bool {
        if !self.tasks.is_empty() {
            return true;
        }
        let now = Instant::now();
        self.timers
            .iter()
            .any(|timer| !timer.fired && timer.when <= now)
    }

    fn adopt_dial_completions(&mut self) {
        for completed in self.dials.adopt() {
            match completed {
                Ok(done) => self.tasks.push_back(Task::DialFinished(Box::new(done))),
                Err(fail) => self.tasks.push_back(Task::DialFailed(fail)),
            }
        }
    }

    /// Earliest timer deadline this document waits for.
    #[must_use]
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.timers
            .iter()
            .filter(|timer| !timer.fired)
            .map(|timer| timer.when)
            .min()
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
        self.dials.reset();
        self.decoder = None;
        self.frame_load_in_flight = false;
        self.shared.borrow_mut().close_frame_ports(self.frame);
        self.world.borrow_mut().forget_owned_documents();
    }

    pub(crate) fn waiting_for_load(&self) -> bool {
        self.dials
            .waiting_for_load(self.js_epoch, self.frame_load_sequence)
            || self.parser_blocked_script.is_some()
            || self.frame_load_in_flight
            || self.active_buffer.is_some()
            || self.world.borrow().main_ready_state() != crate::ReadyState::Complete
    }

    pub(in crate::document) fn launch_queued_dials(&mut self) {
        // A `data:` response can itself request more `data:` images. Keep
        // decoding those before returning so paint does not run with the
        // fetches still sitting on the channel. Network dials stay async.
        // Eight passes is the bound for one drain step, so a `data:` cycle
        // cannot spin inside this call. A chain longer than that stays
        // queued; the wake below asks the session for another step instead
        // of waiting for unrelated work.
        for _ in 0..8 {
            let queued = self.dials.take_queued();
            if queued.is_empty() {
                break;
            }
            let mut completed_data = false;
            for dial in queued {
                // The queue holds the parsed URL's serialization, so the
                // prefix test is the scheme test (see `DialRequest::url`).
                if dial.req.url.starts_with("data:") {
                    completed_data = true;
                    // Same ceiling as a streamed navigation body. Decoding an
                    // unbounded `data:` URL would sit entirely in this process.
                    match crate::engine::decode_data_url_bounded(&dial.req.url) {
                        Some((content_type, body)) => {
                            let QueuedDial {
                                context,
                                req,
                                ancestors,
                                integrity,
                            } = dial;
                            let final_url = req.url.clone();
                            self.finish_dial(super::CompletedDial {
                                context,
                                req,
                                ancestors,
                                integrity,
                                outcome: crate::protocol::DialOutcome {
                                    status: 200,
                                    final_url,
                                    content_type: Some(content_type.clone()),
                                    // Scheme fetch serves the data: MIME type
                                    // as the response's `Content-Type` header
                                    // (<https://fetch.spec.whatwg.org/#concept-scheme-fetch>).
                                    headers: vec![("content-type".to_owned(), content_type)],
                                    body,
                                    response_type: net::cors::ResponseType::Basic,
                                },
                            });
                        }
                        None => self.fail_dial(dial.context),
                    }
                    continue;
                }
                let services = &*self.services;
                self.dials.start(dial, &|request, completion| {
                    services.start_dial(request, completion)
                });
            }
            if !completed_data {
                break;
            }
            self.drain_blitz_fetches();
        }
        if self.dials.has_queued() {
            self.wake.notify_one();
        }
    }

    fn run_task(&mut self, task: Task) {
        match task {
            Task::Timer(id) => {
                self.record_event(RendererEvent::Timer(id));
                if let Some(js_id) = self.js_timer_slots.remove(&id) {
                    self.fire_js(|js| js.fire_timer(js_id));
                }
                self.timers.retain(|timer| timer.id != id);
                self.adopt_js_work();
            }
            Task::DialFinished(done) => self.finish_dial(*done),
            Task::DialFailed(fail) => self.fail_dial(fail),
            Task::WindowMessage(message) => self.deliver_window_message(&message),
            Task::StorageEvent(event) => self.deliver_storage_event(&event),
            Task::RemoteMessage(payload) => self.deliver_remote_message(&payload),
            Task::BroadcastMessage {
                origin,
                name,
                payload,
                source,
            } => self.deliver_broadcast_message(&origin, &name, &payload, source),
            Task::PortMessage {
                endpoint,
                payload,
                ports,
            } => self.deliver_port_message(endpoint, &payload, &ports),
            Task::PortClosed { endpoint } => self.deliver_port_close(endpoint),
            Task::NodeLoad(element) => {
                self.fire_node_load(element);
                self.fire_document_load();
            }
            Task::NodeError(element) => {
                self.fire_js(|js| js.fire_node_error(element));
                self.adopt_js_work();
                self.fire_document_load();
            }
        }
    }

    pub(in crate::document) fn adopt_js_work(&mut self) {
        let history = std::mem::take(&mut self.world.borrow_mut().pending_history);
        for event in history {
            if let RendererEvent::HistoryUpdated { ref url, .. } = event {
                self.apply_document_url_same_origin(url);
            }
            self.record_event(event);
        }
        self.adopt_pending_timers();
        self.adopt_pending_fetches();
        let images = self.world.borrow_mut().take_image_updates();
        for element in images {
            self.queue_image(element, true);
        }
        self.queue_dynamic_scripts();
        let violations = std::mem::take(&mut self.world.borrow_mut().state.pending_csp_violations);
        for (target, violations) in violations {
            self.fire_csp_violations(target, &violations);
        }
    }

    /// Adopts timer starts and cancels from the realm.
    fn adopt_pending_timers(&mut self) {
        let timeouts = self
            .js
            .as_ref()
            .map(crate::js::JsRealm::take_pending_timeouts)
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
    }

    /// Adopts `fetch` calls and cancels from the realm. The connect check
    /// runs here, the one place every page request leaves the renderer,
    /// instead of in each JS API
    /// (<https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>).
    fn adopt_pending_fetches(&mut self) {
        let fetches = self
            .js
            .as_ref()
            .map(crate::js::JsRealm::take_pending_fetches)
            .unwrap_or_default();
        let fetch_cancels: HashSet<_> =
            std::mem::take(&mut self.world.borrow_mut().pending_fetch_cancels)
                .into_iter()
                .collect();
        self.dials.cancel_fetches(self.js_epoch, &fetch_cancels);
        self.queue_js_fetches(fetches, &fetch_cancels);
        let images = self.world.borrow_mut().take_image_updates();
        for element in images {
            self.queue_image(element, true);
        }
        self.queue_dynamic_scripts();
        let violations = std::mem::take(&mut self.world.borrow_mut().state.pending_csp_violations);
        for (target, violations) in violations {
            self.fire_csp_violations(target, &violations);
        }
    }

    /// Connected `<script src>` elements queued their prepare steps from
    /// script; the fetch starts here.
    fn queue_dynamic_scripts(&mut self) {
        let scripts = self.world.borrow_mut().take_pending_scripts();
        for element in scripts {
            let src = self
                .world
                .borrow()
                .with_document(element, |parsed| {
                    crate::dom::tree::attr(&parsed.document.base, element.node, "src")
                        .map(str::to_owned)
                })
                .flatten();
            let Some(src) = src else {
                continue;
            };
            let started = self
                .world
                .borrow()
                .with_document(element, |parsed| {
                    parsed.document.script_already_started(element.node)
                })
                .unwrap_or(false);
            if started {
                continue;
            }
            // Mark at launch: prepare deliberately leaves the flag clear
            // so this queue is the single owner of the fetch. A second
            // prepare sees the flag and schedules nothing.
            if let Some(mut parsed) = self.world.borrow().document_mut(element) {
                parsed.document.mark_script_started(element.node);
            }
            let Ok(url) = self.resolve_dial_url(&src) else {
                self.fire_js(|js| js.fire_node_error(element));
                continue;
            };
            let violations = self.script_url_violations(&url, element);
            if !violations.is_empty() {
                self.fire_csp_violations(element, &violations);
            }
            if crate::csp::blocks(&violations) {
                self.fire_js(|js| js.fire_node_error(element));
                continue;
            }
            let mut dial = QueuedDial::new(
                DialContext::ClassicScript {
                    element,
                    epoch: self.js_epoch,
                },
                &url,
                &self.url,
            );
            dial.req.csp = Some(Box::new(
                self.redirect_gate(crate::csp::Destination::Script),
            ));
            dial.integrity = self.script_integrity(element);
            self.dials.queue(dial);
        }
    }

    /// Queues page `fetch` / XHR dials. The connect check is here, where the
    /// request leaves the renderer
    /// (<https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>).
    fn queue_js_fetches(
        &mut self,
        fetches: Vec<crate::js::PendingJsFetch>,
        fetch_cancels: &HashSet<i32>,
    ) {
        for fetch in fetches {
            if fetch_cancels.contains(&fetch.js_id) {
                continue;
            }
            let pending_js_fetches = self.dials.pending_js_fetches();
            if pending_js_fetches >= MAX_PENDING_JS_FETCHES {
                self.record_event(RendererEvent::FetchFailed);
                self.settle_js_fetch(fetch.js_id, None);
                continue;
            }
            if let Ok(url) = self.resolve_dial_url(&fetch.url) {
                let violations =
                    self.subresource_violations(&url, crate::csp::Destination::Connect);
                if !violations.is_empty()
                    && let Some(target) = self.document_node()
                {
                    self.fire_csp_violations(target, &violations);
                }
                if crate::csp::blocks(&violations) {
                    self.record_event(RendererEvent::FetchFailed);
                    self.settle_js_fetch(fetch.js_id, None);
                    continue;
                }
                let mut dial = QueuedDial::new(
                    DialContext::JsFetch {
                        id: fetch.js_id,
                        epoch: self.js_epoch,
                    },
                    &url,
                    &self.url,
                );
                dial.req.method = fetch.method;
                dial.req.body = fetch.body;
                dial.req.content_type = fetch.content_type;
                dial.req.headers = fetch.headers;
                dial.req.referrer = fetch.referrer;
                dial.req.cors = fetch.cors;
                dial.req.csp = Some(Box::new(
                    self.redirect_gate(crate::csp::Destination::Connect),
                ));
                self.dials.queue(dial);
            } else {
                self.record_event(RendererEvent::FetchFailed);
                self.settle_js_fetch(fetch.js_id, None);
            }
        }
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
