//! One counter for everything that delays a document's `load` event.
//!
//! The counter is the whole gate: a feature that delays `load` takes a guard
//! when it starts and drops the guard when it settles, so it cannot forget to
//! release the event. Engines use the same shape — Ladybird's
//! `DocumentLoadEventDelayer` and Chromium's `IncrementLoadEventDelayCount` —
//! because a new delaying feature joins the count by construction instead of
//! editing a predicate
//! (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The count of things delaying one document's `load` event.
#[derive(Clone, Default)]
pub(crate) struct LoadDelay {
    pending: Arc<AtomicUsize>,
}

impl LoadDelay {
    /// Registers one delaying operation. The guard releases it on drop.
    pub(crate) fn guard(&self) -> LoadDelayGuard {
        self.pending.fetch_add(1, Ordering::Relaxed);
        LoadDelayGuard {
            pending: Arc::clone(&self.pending),
        }
    }

    /// Whether any operation still delays `load`.
    pub(crate) fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed) != 0
    }
}

/// One token in a [`LoadDelay`] count. Dropping it releases the delay.
pub(crate) struct LoadDelayGuard {
    pending: Arc<AtomicUsize>,
}

impl Drop for LoadDelayGuard {
    fn drop(&mut self) {
        let previous = self.pending.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(previous > 0, "load-delay guard dropped without a count");
    }
}

#[cfg(test)]
mod tests {
    use super::LoadDelay;

    #[test]
    fn guards_hold_the_count_until_dropped() {
        let delay = LoadDelay::default();
        assert!(!delay.is_pending());
        let first = delay.guard();
        assert!(delay.is_pending());
        let second = delay.guard();
        drop(first);
        assert!(delay.is_pending());
        drop(second);
        assert!(!delay.is_pending());
    }
}
