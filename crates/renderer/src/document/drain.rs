use std::time::Duration;

use tokio::time::Instant;

use super::{DialContext, Document, MAX_PENDING_JS_FETCHES, QueuedDial, Task, Timer};
use crate::js::RealmRequest;
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
            let fired = self.check_load();
            return !self.stop.is_set()
                && (fired
                    || self.has_pending_frame_loads()
                    || self.has_ready_work());
        }
        let fired = self.check_load();
        !self.stop.is_set() && (fired || self.has_pending_frame_loads())
    }

    /// Whether a task is queued or a timer is already due.
    pub(crate) fn has_ready_work(&self) -> bool {
        if !self.tasks.is_empty() || self.dials.has_queued() {
            return true;
        }
        let now = Instant::now();
        self.timers
            .iter()
            .any(|timer| !timer.fired && timer.when <= now)
    }

    fn adopt_dial_completions(&mut self) {
        for (id, completed) in self.dials.adopt() {
            match completed {
                Ok(done) => self.tasks.push_back(Task::DialFinished(id, Box::new(done))),
                Err(fail) => self.tasks.push_back(Task::DialFailed(id, fail)),
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
        let old = std::mem::replace(
            &mut self.dials,
            super::DialLedger::new(
                std::sync::Arc::clone(&self.stop),
                std::sync::Arc::clone(&self.wake),
            ),
        );
        old.terminate();
        self.decoder = None;
        self.shared.borrow_mut().close_frame_ports(self.frame);
        self.world.borrow_mut().forget_owned_documents();
    }

    pub(crate) fn waiting_for_load(&self) -> bool {
        self.world.borrow().main_ready_state() != crate::ReadyState::Complete
    }

    fn has_pending_frame_loads(&self) -> bool {
        !self.pending_frame_loads.is_empty()
    }

    pub(in crate::document) fn launch_queued_dials(&mut self) {
        // A `data:` response can itself request more `data:` images. Keep
        // decoding those before returning so paint does not run with the
        // fetches still sitting on the queue. Eight passes is the bound for one
        // drain step, so a `data:` cycle cannot spin inside this call.
        for _ in 0..8 {
            let mut completed_data = false;
            // One at a time: a `data:` frame load can commit a new document
            // and replace the ledger, and the rest of the old queue must not
            // start on the new one.
            while let Some(entry) = self.dials.pop_queued() {
                let super::dials::QueuedEntry { dial, delay } = entry;
                // The queue holds the parsed URL's serialization, so the
                // prefix test is the scheme test (see `DialRequest::url`).
                if dial.req.url.starts_with("data:") {
                    completed_data = true;
                    // Same ceiling as a streamed navigation body. Decoding an
                    // unbounded `data:` URL would sit entirely in this process.
                    if let Some((content_type, body)) =
                        crate::engine::decode_data_url_bounded(&dial.req.url)
                    {
                        drop(delay);
                        let final_url = dial.req.url.clone();
                        self.finish_dial(super::CompletedDial {
                            context: dial.context,
                            req: dial.req,
                            ancestors: dial.ancestors,
                            integrity: dial.integrity,
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
                    } else {
                        drop(delay);
                        self.fail_dial(dial.context);
                    }
                    continue;
                }
                let services = &*self.services;
                self.dials.start(dial, delay, &|request, completion| {
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
                if let Some(js_id) = self.timer_js_ids.remove(&id) {
                    self.js_timers.remove(&js_id);
                    self.fire_js(|js| js.fire_timer(js_id));
                }
                self.timers.retain(|timer| timer.id != id);
                self.adopt_js_work();
            }
            Task::DialFinished(id, done) => {
                if let Some(dial) = self.dials.take_finished(id) {
                    dial.settle();
                    self.finish_dial(*done);
                }
            }
            Task::DialFailed(id, fail) => {
                if let Some(dial) = self.dials.take_finished(id) {
                    dial.settle();
                    self.fail_dial(fail);
                }
            }
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
            }
            Task::NodeError(element) => {
                self.fire_js(|js| js.fire_node_error(element));
                self.adopt_js_work();
            }
        }
    }

    /// Drains the realm's outbox. Requests are processed in program order,
    /// including any they generate while they run, so one pass is the whole
    /// handoff.
    pub(in crate::document) fn adopt_js_work(&mut self) {
        loop {
            let requests = self.world.borrow_mut().take_outbox();
            if requests.is_empty() {
                break;
            }
            for request in requests {
                self.apply_realm_request(request);
            }
        }
    }

    fn apply_realm_request(&mut self, request: RealmRequest) {
        match request {
            RealmRequest::Timer(timeout) => {
                let page_id = self.schedule_timer(timeout.delay);
                self.js_timers.insert(timeout.js_id, page_id);
                self.timer_js_ids.insert(page_id, timeout.js_id);
            }
            // Keyed by the script-visible id, so a cancel is a map remove
            // instead of a scan over every live timer.
            RealmRequest::ClearTimer(js_id) => {
                if let Some(page_id) = self.js_timers.remove(&js_id) {
                    self.timer_js_ids.remove(&page_id);
                    self.timers.retain(|timer| timer.id != page_id);
                }
            }
            // The connect check runs here, the one place every page request
            // leaves the renderer, instead of in each JS API
            // (<https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>).
            RealmRequest::Fetch(fetch) => self.queue_js_fetch(fetch),
            RealmRequest::AbortFetch(js_id) => self.dials.cancel_fetch(js_id),
            RealmRequest::Script(element) => self.queue_dynamic_script(element),
            RealmRequest::Image(element) => self.queue_image(element, true),
            RealmRequest::History(event) => {
                if let RendererEvent::HistoryUpdated { ref url, .. } = event {
                    self.apply_document_url_same_origin(url);
                }
                self.record_event(event);
            }
            RealmRequest::CspViolation(target, violations) => {
                self.fire_csp_violations(target, &violations);
            }
            RealmRequest::HtmlWrite(html) => {
                // The binding reserved these bytes before queueing the write.
                self.world.borrow().release_stream_bytes(html.len());
                self.write_text(&html);
            }
        }
    }

    /// A connected `<script src>` element queued its prepare steps from
    /// script; the fetch starts here.
    fn queue_dynamic_script(&mut self, element: crate::dom::tree::NodeId) {
        let src = self
            .world
            .borrow()
            .with_document(element, |parsed| {
                crate::dom::tree::attr(&parsed.document.base, element.node, "src")
                    .map(str::to_owned)
            })
            .flatten();
        let Some(src) = src else {
            return;
        };
        let started = self
            .world
            .borrow()
            .with_document(element, |parsed| {
                parsed.document.script_already_started(element.node)
            })
            .unwrap_or(false);
        if started {
            return;
        }
        // Mark at launch: prepare deliberately leaves the flag clear
        // so this queue is the single owner of the fetch. A second
        // prepare sees the flag and schedules nothing.
        if let Some(mut parsed) = self.world.borrow().document_mut(element) {
            parsed.document.mark_script_started(element.node);
        }
        let Ok(url) = self.resolve_dial_url(&src) else {
            self.fire_js(|js| js.fire_node_error(element));
            return;
        };
        let violations = self.script_url_violations(&url, element);
        if !violations.is_empty() {
            self.fire_csp_violations(element, &violations);
        }
        if crate::csp::blocks(&violations) {
            self.fire_js(|js| js.fire_node_error(element));
            return;
        }
        let mut dial = QueuedDial::new(
            DialContext::ClassicScript { element },
            &url,
            &self.url,
        );
        dial.req.csp = Some(Box::new(
            self.redirect_gate(crate::csp::Destination::Script),
        ));
        dial.integrity = self.script_integrity(element);
        // A script-inserted `src` script is async, but it still belongs to
        // the set of scripts that will execute as soon as possible, and
        // `load` waits for that set to drain
        // (<https://html.spec.whatwg.org/multipage/parsing.html#the-end>).
        dial.delay = Some(self.load_delay.guard());
        self.dials.queue(dial);
    }

    /// Queues one page `fetch` / XHR dial. The connect check is here, where
    /// the request leaves the renderer
    /// (<https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>).
    fn queue_js_fetch(&mut self, fetch: crate::js::PendingJsFetch) {
        let pending_js_fetches = self.dials.pending_js_fetches();
        if pending_js_fetches >= MAX_PENDING_JS_FETCHES {
            self.record_event(RendererEvent::FetchFailed);
            self.settle_js_fetch(fetch.js_id, None);
            return;
        }
        if let Ok(url) = self.resolve_dial_url(&fetch.url) {
            let violations = self.subresource_violations(&url, crate::csp::Destination::Connect);
            if !violations.is_empty()
                && let Some(target) = self.document_node()
            {
                self.fire_csp_violations(target, &violations);
            }
            if crate::csp::blocks(&violations) {
                self.record_event(RendererEvent::FetchFailed);
                self.settle_js_fetch(fetch.js_id, None);
                return;
            }
            let mut dial = QueuedDial::new(
                DialContext::JsFetch { id: fetch.js_id },
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
            self.dials.queue(dial, &self.load_delay);
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
