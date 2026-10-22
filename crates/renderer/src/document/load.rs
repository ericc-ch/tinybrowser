//! One counter for everything that delays a document's `load` event.
//!
//! A delaying operation takes a guard when its request is born and drops it
//! when the request settles
//! (<https://html.spec.whatwg.org/multipage/parsing.html#delay-the-load-event>).
//! The document checks the count through one method, [`LoadDelay::is_pending`].

use std::cell::Cell;
use std::rc::Rc;

/// The count of things delaying one document's `load` event.
#[derive(Default)]
pub(crate) struct LoadDelay {
    pending: Rc<Cell<usize>>,
}

impl LoadDelay {
    /// Registers one delaying operation. The guard releases it on drop.
    pub(crate) fn guard(&self) -> LoadDelayGuard {
        self.pending.set(self.pending.get() + 1);
        LoadDelayGuard {
            pending: Rc::clone(&self.pending),
        }
    }

    /// Whether any operation still delays `load`.
    pub(crate) fn is_pending(&self) -> bool {
        self.pending.get() != 0
    }
}

/// One token in a [`LoadDelay`] count. Dropping it releases the delay.
pub(crate) struct LoadDelayGuard {
    pending: Rc<Cell<usize>>,
}

impl Drop for LoadDelayGuard {
    fn drop(&mut self) {
        self.pending.set(self.pending.get() - 1);
    }
}
