//! Mutation records and the structural change serial.

use super::{Mutation, MutationOrder};

#[derive(Debug, Default)]
pub(crate) struct MutationJournal {
    mutations: Vec<(u64, Mutation)>,
    order: MutationOrder,
    recording: bool,
    suppressed: bool,
    serial: u64,
}

impl MutationJournal {
    pub(super) fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
        if !recording {
            self.mutations.clear();
        }
    }

    pub(super) fn take(&mut self) -> Vec<Mutation> {
        self.take_ordered()
            .into_iter()
            .map(|(_, mutation)| mutation)
            .collect()
    }

    pub(super) fn take_ordered(&mut self) -> Vec<(u64, Mutation)> {
        std::mem::take(&mut self.mutations)
    }

    pub(super) fn share_order(&mut self, order: &MutationOrder) {
        if std::sync::Arc::ptr_eq(&self.order.0, &order.0) {
            return;
        }
        self.order = order.clone();
        for (position, _) in &mut self.mutations {
            *position = self.order.next();
        }
    }

    pub(super) fn record(&mut self, mutation: Mutation) {
        self.bump();
        if self.recording() {
            self.mutations.push((self.order.next(), mutation));
        }
    }

    pub(super) fn bump(&mut self) {
        self.serial = self.serial.wrapping_add(1);
    }

    pub(super) fn serial(&self) -> u64 {
        self.serial
    }

    pub(super) fn recording(&self) -> bool {
        self.recording && !self.suppressed
    }

    pub(super) fn suppress(&mut self, suppressed: bool) {
        self.suppressed = suppressed;
    }
}
