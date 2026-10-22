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
//! The ledger owns the completion channel, so no other module sees a sender or
//! receiver. Callers queue a request, start it with the browser's dial
//! function, and collect finished dials; the counting and cancellation
//! bookkeeping stays here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use tokio::sync::Notify;

use super::{CompletedDial, DialContext, LoadDelayGuard, QueuedDial};
use crate::js::Stop;
use crate::protocol::{DialCancellation, DialCompletion, DialRequest};

/// One started dial and the resources it releases on settle.
struct InFlightDial {
    /// The context callers count against (`fetch` concurrency) and cancel.
    context: DialContext,
    /// Stops the request, including its redirect chain. Calling it still
    /// delivers exactly one completion.
    cancel: DialCancellation,
    /// The `load`-event delay token, held until the completion is adopted or
    /// the ledger is terminated.
    _delay: Option<LoadDelayGuard>,
}

pub(crate) struct DialLedger {
    /// Completions from every started dial, keyed by ledger entry so adoption
    /// frees the right cancellation handle. Both ends stay private: a caller
    /// that could clone the sender could outlive the ledger's accounting.
    tx: Sender<(u64, Result<CompletedDial, DialContext>)>,
    rx: Receiver<(u64, Result<CompletedDial, DialContext>)>,
    /// Dials waiting for the drain to start them.
    queued: Vec<QueuedDial>,
    /// Started dials, keyed by a ledger-local id.
    in_flight: HashMap<u64, InFlightDial>,
    /// Dial ids whose cancellation was requested, so a completion delivered
    /// for a cancelled dial is not mistaken for the current document's work.
    canceled: HashSet<u64>,
    next_id: u64,
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
            queued: Vec::new(),
            in_flight: HashMap::new(),
            canceled: HashSet::new(),
            next_id: 0,
            stop,
            wake,
        }
    }

    /// Queues one dial for the next drain step.
    pub(crate) fn queue(&mut self, dial: QueuedDial) {
        self.queued.push(dial);
    }

    /// Takes every queued dial. The caller starts them, including any that
    /// complete without the network (`data:`).
    pub(crate) fn take_queued(&mut self) -> Vec<QueuedDial> {
        std::mem::take(&mut self.queued)
    }

    pub(crate) fn has_queued(&self) -> bool {
        !self.queued.is_empty()
    }

    /// Starts one network dial and files its cancellation.
    ///
    /// The completion closure sends into this ledger's channel and wakes the
    /// session; `start` is the browser's dial function, so the ledger never
    /// names the service trait and can be driven without a browser.
    pub(crate) fn start(
        &mut self,
        mut dial: QueuedDial,
        start: &dyn Fn(DialRequest, DialCompletion) -> DialCancellation,
    ) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let completed = self.tx.clone();
        let wake = Arc::clone(&self.wake);
        let stop = Arc::clone(&self.stop);
        let context = dial.context;
        let request = dial.req.clone();
        // The guard belongs to the ledger entry from here on: the completion
        // runs on the carrier thread and cannot hold a document token.
        let delay = dial.delay.take();
        let cancel = start(
            request,
            Box::new(move |outcome| {
                if stop.is_set() {
                    return;
                }
                let result = super::dial::complete(dial, outcome);
                let _send_result = completed.send((id, result));
                wake.notify_one();
            }),
        );
        self.in_flight.insert(
            id,
            InFlightDial {
                context,
                cancel,
                _delay: delay,
            },
        );
    }

    /// Collects finished dials in completion order (oldest first) and drops
    /// their cancellation handles.
    pub(crate) fn adopt(&mut self) -> Vec<Result<CompletedDial, DialContext>> {
        let mut completed = Vec::new();
        while let Ok((id, outcome)) = self.rx.try_recv() {
            // Dropping the entry releases its load-event delay before the
            // document handles the completion.
            self.in_flight.remove(&id);
            if self.canceled.remove(&id) {
                continue;
            }
            completed.push(outcome);
        }
        completed
    }

    /// Page fetches in flight or waiting to start. Only page fetches count:
    /// navigations, scripts, images, and reports have their own budgets, so
    /// a busy document does not make `fetch` fail early.
    pub(crate) fn pending_js_fetches(&self) -> usize {
        self.in_flight
            .values()
            .filter(|dial| matches!(dial.context, DialContext::JsFetch { .. }))
            .count()
            + self
                .queued
                .iter()
                .filter(|dial| matches!(dial.context, DialContext::JsFetch { .. }))
                .count()
    }

    /// Drops queued and in-flight page fetches for `ids`, running their
    /// cancellation handles.
    pub(crate) fn cancel_fetches(&mut self, ids: &HashSet<i32>) {
        self.queued.retain(|dial| {
            !matches!(dial.context, DialContext::JsFetch { id } if ids.contains(&id))
        });
        let matching: Vec<u64> = self
            .in_flight
            .iter()
            .filter(|(_, dial)| {
                matches!(dial.context, DialContext::JsFetch { id } if ids.contains(&id))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in matching {
            if let Some(dial) = self.in_flight.remove(&id) {
                self.canceled.insert(id);
                (dial.cancel)();
            }
        }
    }

    /// Cancels a previous frame navigation: the newest load owns the frame.
    pub(crate) fn cancel_frame_loads(&mut self) {
        self.queued
            .retain(|dial| !matches!(dial.context, DialContext::FrameLoad { .. }));
        let matching: Vec<u64> = self
            .in_flight
            .iter()
            .filter(|(_, dial)| matches!(dial.context, DialContext::FrameLoad { .. }))
            .map(|(id, _)| *id)
            .collect();
        for id in matching {
            if let Some(dial) = self.in_flight.remove(&id) {
                self.canceled.insert(id);
                (dial.cancel)();
            }
        }
    }

    /// Aborts the whole document: every queued and started dial is cancelled
    /// and its `load`-event delay is released. The caller replaces the ledger,
    /// so late completions have no receiver left to reach.
    pub(crate) fn terminate(&mut self) {
        self.queued.clear();
        let in_flight = std::mem::take(&mut self.in_flight);
        for (_, dial) in in_flight {
            (dial.cancel)();
        }
    }
}
