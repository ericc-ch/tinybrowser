//! One document's dial ledger: what is queued, what is in flight, and every
//! cancellation handle.
//!
//! The ledger is the document's fetch group. Every started dial of every kind
//! files its cancellation handle here, so aborting the document terminates
//! classic scripts, frame loads, stylesheets, images, and page `fetch` alike
//! (<https://html.spec.whatwg.org/multipage/document-lifecycle.html#abort-a-document>,
//! <https://fetch.spec.whatwg.org/#concept-fetch-group>). A navigation replaces
//! the ledger, so the new document cannot adopt a completion from the old one.
//!
//! The ledger owns the completion channel and the `load`-event delay policy, so
//! no other module sees a sender, receiver, or guard.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use tokio::sync::Notify;

use super::{CompletedDial, DialContext, LoadDelay, LoadDelayGuard, QueuedDial};
use crate::js::Stop;
use crate::protocol::{DialCancellation, DialCompletion, DialRequest};

/// One queued dial and the load-event delay its request already holds.
pub(crate) struct QueuedEntry {
    pub(crate) dial: QueuedDial,
    pub(crate) delay: Option<LoadDelayGuard>,
}

/// One started dial and the resources it releases on settle.
pub(crate) struct InFlightDial {
    context: DialContext,
    delay: Option<LoadDelayGuard>,
    cancel: DialCancellation,
}

impl InFlightDial {
    /// Releases the load-event delay once the document has run the completion
    /// task.
    pub(crate) fn settle(self) {
        drop(self.delay);
    }
}

pub(crate) struct DialLedger {
    /// Completions from every started dial, keyed by ledger entry so adoption
    /// frees the right cancellation handle. Both ends stay private: a caller
    /// that could clone the sender could outlive the ledger's accounting.
    tx: Sender<(u64, Result<CompletedDial, DialContext>)>,
    rx: Receiver<(u64, Result<CompletedDial, DialContext>)>,
    /// Dials waiting for the drain to start them.
    queued: VecDeque<QueuedEntry>,
    /// Started dials, keyed by a ledger-local id. An entry stays here until its
    /// completion task runs, so cancellation can still find adopted work.
    in_flight: HashMap<u64, InFlightDial>,
    next_id: u64,
    js_fetches: usize,
    queued_js_fetches: usize,
    /// A frame that is shutting down drops completions instead of waking the
    /// session for work nobody will run.
    stop: Arc<Stop>,
    /// Wakes the session loop when a dial finishes.
    wake: Arc<Notify>,
}

impl DialLedger {
    pub(crate) fn new(stop: Arc<Stop>, wake: Arc<Notify>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            queued: VecDeque::new(),
            in_flight: HashMap::new(),
            next_id: 0,
            js_fetches: 0,
            queued_js_fetches: 0,
            stop,
            wake,
        }
    }

    /// Queues one dial for the next drain step. The ledger mints its load-event
    /// delay here, so the policy is one exhaustive match and a new context
    /// cannot silently stop delaying `load`.
    pub(crate) fn queue(&mut self, dial: QueuedDial, delay: &LoadDelay) {
        let guard = delays_load(dial.context).then(|| delay.guard());
        if matches!(dial.context, DialContext::JsFetch { .. }) {
            self.queued_js_fetches += 1;
        }
        self.queued.push_back(QueuedEntry { dial, delay: guard });
    }

    /// Pops one queued dial. A `data:` completion can reset the document and
    /// replace this ledger, so the caller must not hold a batch across it.
    pub(crate) fn pop_queued(&mut self) -> Option<QueuedEntry> {
        let entry = self.queued.pop_front()?;
        if matches!(entry.dial.context, DialContext::JsFetch { .. }) {
            self.queued_js_fetches -= 1;
        }
        Some(entry)
    }

    pub(crate) fn has_queued(&self) -> bool {
        !self.queued.is_empty()
    }

    /// Starts one network dial and files its cancellation and delay.
    pub(crate) fn start(
        &mut self,
        dial: QueuedDial,
        delay: Option<LoadDelayGuard>,
        start: &dyn Fn(DialRequest, DialCompletion) -> DialCancellation,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        let completed = self.tx.clone();
        let wake = Arc::clone(&self.wake);
        let stop = Arc::clone(&self.stop);
        let context = dial.context;
        if matches!(context, DialContext::JsFetch { .. }) {
            self.js_fetches += 1;
        }
        let request = dial.req.clone();
        let cancel = start(
            request,
            Box::new(move |outcome| {
                if stop.is_set() {
                    return;
                }
                let result = super::dial::complete(dial, outcome);
                if completed.send((id, result)).is_ok() {
                    wake.notify_one();
                }
            }),
        );
        self.in_flight.insert(
            id,
            InFlightDial {
                context,
                delay,
                cancel,
            },
        );
    }

    /// Collects finished dials in completion order (oldest first). The
    /// `InFlightDial` stays filed, so its guard still delays `load` until the
    /// task that handles the completion actually runs.
    pub(crate) fn adopt(&mut self) -> Vec<(u64, Result<CompletedDial, DialContext>)> {
        let mut completed = Vec::new();
        while let Ok((id, outcome)) = self.rx.try_recv() {
            if !self.in_flight.contains_key(&id) {
                continue;
            }
            completed.push((id, outcome));
        }
        completed
    }

    /// Removes a dial whose completion task is running. `None` means the dial
    /// was cancelled after its completion was adopted.
    pub(crate) fn take_finished(&mut self, id: u64) -> Option<InFlightDial> {
        let dial = self.in_flight.remove(&id)?;
        if matches!(dial.context, DialContext::JsFetch { .. }) {
            self.js_fetches -= 1;
        }
        Some(dial)
    }

    /// Page fetches in flight, queued, or waiting for their completion task.
    pub(crate) fn pending_js_fetches(&self) -> usize {
        self.js_fetches + self.queued_js_fetches
    }

    /// Drops a queued or in-flight page fetch for one JS id, running its
    /// cancellation handle.
    pub(crate) fn cancel_fetch(&mut self, id: i32) {
        self.cancel_where(|context| {
            matches!(context, DialContext::JsFetch { id: queued } if *queued == id)
        });
    }

    /// Cancels a previous frame navigation: the newest load owns the frame.
    pub(crate) fn cancel_frame_loads(&mut self) {
        self.cancel_where(|context| matches!(context, DialContext::FrameLoad { .. }));
    }

    fn cancel_where(&mut self, matches: impl Fn(&DialContext) -> bool) {
        let queued_js = self
            .queued
            .iter()
            .filter(|entry| {
                matches!(&entry.dial.context, DialContext::JsFetch { .. })
                    && matches(&entry.dial.context)
            })
            .count();
        self.queued_js_fetches -= queued_js;
        self.queued.retain(|entry| !matches(&entry.dial.context));
        let mut removed_js = 0;
        for (_, dial) in self.in_flight.extract_if(|_, dial| matches(&dial.context)) {
            if matches!(dial.context, DialContext::JsFetch { .. }) {
                removed_js += 1;
            }
            (dial.cancel)();
        }
        self.js_fetches -= removed_js;
    }

    /// Aborts the whole document: every queued and started dial is cancelled
    /// and its `load`-event delay is released.
    pub(crate) fn terminate(mut self) {
        self.queued.clear();
        for (_, dial) in std::mem::take(&mut self.in_flight) {
            (dial.cancel)();
        }
    }
}

/// Whether one dial delays its document's `load` event. Images are held by
/// their DOM waiter until decode instead, so the dial itself does not count.
fn delays_load(context: DialContext) -> bool {
    match context {
        DialContext::JsFetch { .. }
        | DialContext::CspReport
        | DialContext::BlitzResource {
            destination: Some(crate::csp::Destination::Img),
            ..
        } => false,
        DialContext::ClassicScript { .. }
        | DialContext::FrameLoad { .. }
        | DialContext::BlitzResource { .. } => true,
    }
}
