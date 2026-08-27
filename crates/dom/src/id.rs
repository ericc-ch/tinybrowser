//! Node identity: copyable handles into the arena.

/// Handle to one node in a [`crate::Dom`].
///
/// A `NodeId` is three integers: which document it was issued by, which slot
/// of that arena to look in, and which generation of that slot. Destroying a
/// node recycles the slot and ticks generation, so an old handle reports
/// "gone" instead of naming a stranger in the same cell. The document id
/// stops a handle from document A from naming a live node in document B that
/// happens to share slot and generation.
///
/// Copy it freely, store it anywhere, hand it to JavaScript later. The only
/// thing you can do with it is pass it back to the [`crate::Dom`] it came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId {
    pub(crate) document: u32,
    pub(crate) slot: u32,
    pub(crate) generation: u32,
}

impl NodeId {
    pub(crate) const fn new(document: u32, slot: u32, generation: u32) -> Self {
        Self {
            document,
            slot,
            generation,
        }
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
