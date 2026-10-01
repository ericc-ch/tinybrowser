//! Agent-owned mutation registrations and notification state.

use std::collections::BTreeMap;

use rquickjs::{Function, Object, Persistent};

use crate::documents::DocumentStore;
use crate::protocol::FrameId;

use super::world::{
    NodeReference, Observation, ObserverOptions, ObserverState, ReadyObserver, RecordData,
    match_observation,
};

/// One pending-observer set and notification flag for one window agent
/// (<https://dom.spec.whatwg.org/#mutation-observers>).
#[derive(Default)]
pub(crate) struct MutationObservers {
    observers: BTreeMap<u64, ObserverState>,
    next_id: u64,
    next_registration: u64,
    pending: Vec<u64>,
    pub(crate) scheduled: bool,
}

impl MutationObservers {
    pub(crate) fn create(
        &mut self,
        owner: FrameId,
        callback: Persistent<Function<'static>>,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.observers.insert(
            id,
            ObserverState {
                owner,
                callback,
                object: None,
                observations: Vec::new(),
                queue: Vec::new(),
            },
        );
        id
    }

    // https://dom.spec.whatwg.org/#dom-mutationobserver-observe
    pub(crate) fn observe(
        &mut self,
        id: u64,
        target: NodeReference,
        options: ObserverOptions,
        object: Persistent<Object<'static>>,
        documents: &mut DocumentStore,
    ) {
        self.drain(documents);
        if let Some(observer) = self.observers.get_mut(&id) {
            if let Some(existing) = observer
                .observations
                .iter_mut()
                .find(|item| item.target == target)
            {
                existing.options = options;
            } else {
                self.next_registration += 1;
                observer.observations.push(Observation {
                    target,
                    options,
                    order: self.next_registration,
                });
            }
            observer.object = Some(object);
            if let Some(parsed) = documents.get_mut(target.scope().document_id()) {
                dom::mutation::set_recording(&mut parsed.document, true);
            }
        }
    }

    // https://dom.spec.whatwg.org/#dom-mutationobserver-disconnect
    pub(crate) fn disconnect(&mut self, id: u64, documents: &mut DocumentStore) {
        self.drain(documents);
        if let Some(observer) = self.observers.get_mut(&id) {
            observer.queue.clear();
            observer.observations.clear();
            observer.object = None;
        }
        self.refresh_recording(documents);
    }

    // https://dom.spec.whatwg.org/#dom-mutationobserver-takerecords
    pub(crate) fn take_records(
        &mut self,
        id: u64,
        documents: &mut DocumentStore,
    ) -> Vec<RecordData> {
        self.drain(documents);
        self.observers
            .get_mut(&id)
            .map(|observer| std::mem::take(&mut observer.queue))
            .unwrap_or_default()
    }

    pub(crate) fn forget_frame(&mut self, owner: FrameId, documents: &mut DocumentStore) {
        self.drain(documents);
        self.observers.retain(|_, observer| observer.owner != owner);
        self.pending.retain(|id| self.observers.contains_key(id));
        self.refresh_recording(documents);
    }

    fn refresh_recording(&self, documents: &mut DocumentStore) {
        for parsed in documents.values_mut() {
            let id = parsed.document.document_id();
            let watched = self.observers.values().any(|observer| {
                observer
                    .observations
                    .iter()
                    .any(|item| item.target.scope().document_id() == id)
            });
            dom::mutation::set_recording(&mut parsed.document, watched);
        }
    }

    // https://dom.spec.whatwg.org/#queue-a-mutation-record
    pub(crate) fn drain(&mut self, documents: &mut DocumentStore) {
        let mut mutations = Vec::new();
        for parsed in documents.values_mut() {
            let id = parsed.document.document_id();
            mutations.extend(
                dom::mutation::take_ordered(&mut parsed.document)
                    .into_iter()
                    .map(|(position, mutation)| (position, id, mutation)),
            );
        }
        mutations.sort_unstable_by_key(|(position, _, _)| *position);
        for (_, document, mutation) in mutations {
            if let Some(parsed) = documents.get(document) {
                let mut interested: Vec<_> = self
                    .observers
                    .iter()
                    .filter_map(|(&id, observer)| {
                        match_observation(&parsed.document, observer, &mutation)
                            .map(|(depth, order, record)| (depth, order, id, record))
                    })
                    .collect();
                // DOM inserts interested observers in ancestor/registration
                // order (#queue-a-mutation-record). Chromium instead sorts
                // notification by creation priority; retain the spec order.
                interested.sort_by_key(|(depth, order, _, _)| (*depth, *order));
                for (_, _, id, record) in interested {
                    if let Some(observer) = self.observers.get_mut(&id) {
                        observer.queue.push(record);
                    }
                    if !self.pending.contains(&id) {
                        self.pending.push(id);
                    }
                }
            }
        }
    }

    pub(crate) fn needs_notification(&self) -> bool {
        !self.scheduled && !self.pending.is_empty()
    }

    // https://dom.spec.whatwg.org/#notify-mutation-observers
    pub(crate) fn begin_notification(&mut self, documents: &mut DocumentStore) -> Vec<u64> {
        self.drain(documents);
        self.scheduled = false;
        std::mem::take(&mut self.pending)
    }

    pub(crate) fn delivery(&mut self, id: u64) -> Option<ReadyObserver> {
        let observer = self.observers.get_mut(&id)?;
        let records = std::mem::take(&mut observer.queue);
        if records.is_empty() {
            return None;
        }
        Some(ReadyObserver {
            callback: observer.callback.clone(),
            object: observer.object.clone()?,
            records,
        })
    }
}
