//! Node identity: copyable handles into the arena.

/// Handle to one node in a [`crate::Dom`].
///
/// A `NodeId` is two integers: which slot of the arena to look in, and which
/// generation of that slot the handle was issued for. When a node is destroyed
/// its slot is recycled and the slot's generation ticks, so any handle issued
/// earlier stops matching; every lookup then reports "gone" instead of
/// returning some other node.
///
/// Copy it freely, store it anywhere, hand it to JavaScript later. The only
/// thing you can do with it is pass it back to the [`crate::Dom`] it came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId {
    pub(crate) slot: u32,
    pub(crate) generation: u32,
}

impl NodeId {
    pub(crate) const fn new(slot: u32, generation: u32) -> Self {
        Self { slot, generation }
    }

    /// Slot index as an array position. Only valid when
    /// `slot < slots.len()`, which holds exactly when the handle is live.
    ///
    /// The cast is a lossless widening (`u32` → pointer-width `usize`);
    /// no `From` impl exists for that direction, so `as` is the honest form.
    pub(crate) fn index(self) -> usize {
        self.slot as usize
    }
}
