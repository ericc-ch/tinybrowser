//! Mutation records and the structural change serial.

use super::Mutation;

#[derive(Debug, Default)]
pub(crate) struct MutationJournal {
    mutations: Vec<Mutation>,
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
        std::mem::take(&mut self.mutations)
    }

    pub(super) fn record(&mut self, mutation: Mutation) {
        self.bump();
        if self.recording() {
            self.mutations.push(mutation);
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
