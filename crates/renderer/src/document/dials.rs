//! One document's dial ledger: what is queued, what is in flight, and the
//! cancellation handles for page fetches.
//!
//! The ledger owns the completion channel, so no other module sees a sender or
//! receiver. Callers queue a request, start it with the browser's dial
//! function, and collect finished dials; the counting and cancellation
//! bookkeeping stays here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use tokio::sync::Notify;

use super::{CompletedDial, DialContext, QueuedDial};
use crate::js::Stop;
use crate::protocol::{DialCancellation, DialCompletion, DialRequest};

pub(crate) struct DialLedger {
    /// Completions from every started dial. Both ends stay private: a caller
    /// that could clone the sender could outlive the ledger's accounting.
    tx: Sender<Result<CompletedDial, DialContext>>,
    rx: Receiver<Result<CompletedDial, DialContext>>,
    /// Dials waiting for the drain to start them.
    queued: Vec<QueuedDial>,
    /// Started dials whose completion has not arrived yet.
    in_flight: usize,
    /// Page-fetch cancellation handles, keyed by `(epoch, fetch id)`.
    cancellations: HashMap<(u64, i32), DialCancellation>,
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
            in_flight: 0,
            cancellations: HashMap::new(),
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
        dial: QueuedDial,
        start: &dyn Fn(DialRequest, DialCompletion) -> DialCancellation,
    ) {
        let completed = self.tx.clone();
        let wake = Arc::clone(&self.wake);
        let stop = Arc::clone(&self.stop);
        let context = dial.context;
        let request = dial.req.clone();
        self.in_flight = self.in_flight.saturating_add(1);
        let cancel = start(
            request,
            Box::new(move |outcome| {
                if stop.is_set() {
                    return;
                }
                let result = super::dial::complete(dial, outcome);
                let _send_result = completed.send(result);
                wake.notify_one();
            }),
        );
        if let DialContext::JsFetch { epoch, id } = context {
            self.cancellations.insert((epoch, id), cancel);
        }
    }

    /// Collects finished dials, newest first, and drops their cancellation
    /// handles.
    pub(crate) fn adopt(&mut self) -> Vec<Result<CompletedDial, DialContext>> {
        let mut completed = Vec::new();
        while let Ok(outcome) = self.rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            let context = match &outcome {
                Ok(done) => done.context,
                Err(context) => *context,
            };
            if let DialContext::JsFetch { epoch, id } = context {
                self.cancellations.remove(&(epoch, id));
            }
            completed.push(outcome);
        }
        completed
    }

    /// Page fetches in flight or waiting to start. Non-fetch dials count
    /// toward the in-flight total, exactly as the old ledger did.
    pub(crate) fn pending_js_fetches(&self) -> usize {
        self.in_flight
            + self
                .queued
                .iter()
                .filter(|dial| matches!(dial.context, DialContext::JsFetch { .. }))
                .count()
    }

    /// Drops queued and in-flight page fetches for `ids`, running their
    /// cancellation handles.
    pub(crate) fn cancel_fetches(&mut self, epoch: u64, ids: &HashSet<i32>) {
        self.queued.retain(|dial| {
            !matches!(
                dial.context,
                DialContext::JsFetch { id, epoch: queued_epoch }
                    if queued_epoch == epoch && ids.contains(&id)
            )
        });
        for id in ids {
            if let Some(cancel) = self.cancellations.remove(&(epoch, *id)) {
                cancel();
            }
        }
    }

    /// Whether a dial from the current script epoch or frame load is still
    /// queued, which delays the document's `load` event.
    pub(crate) fn waiting_for_load(&self, js_epoch: u64, frame_sequence: u64) -> bool {
        self.queued.iter().any(|dial| match &dial.context {
            DialContext::ClassicScript { epoch, .. } => *epoch == js_epoch,
            DialContext::FrameLoad { sequence, .. } => *sequence == frame_sequence,
            // Blitz tracks its own critical resources; ours never gate load.
            // Reports are fire-and-forget and never gate load either.
            DialContext::BlitzResource { .. }
            | DialContext::JsFetch { .. }
            | DialContext::CspReport => false,
        })
    }

    /// Cancels page-fetch handles and drops queued dials. Dials already
    /// started keep their slot in `in_flight`, because their completions still
    /// arrive and must decrement it.
    pub(crate) fn discard_queued(&mut self) {
        for (_, cancel) in self.cancellations.drain() {
            cancel();
        }
        self.queued.clear();
    }

    /// Cancels every handle, drops all queued work, and forgets the in-flight
    /// count. The document is being torn down and no completion will be
    /// adopted.
    pub(crate) fn reset(&mut self) {
        self.discard_queued();
        self.in_flight = 0;
    }
}
