//! Per-node children storage: inline up to four IDs, spilling once to the heap.

use crate::id::NodeId;

/// Inline capacity before a node's children list spills to the heap.
///
/// Real HTML skews small: most elements hold one to three children. Wide
/// containers spill once and behave like an ordinary growable buffer from
/// then on — spilled lists never move back inline, so churny pages cannot
/// thrash between representations.
const INLINE_CAP: usize = 4;

/// Children of one node, in document order.
#[derive(Clone, Debug)]
pub(crate) enum Children {
    /// Up to [`INLINE_CAP`] handles stored inside the node record itself.
    Inline { len: u8, ids: [NodeId; INLINE_CAP] },
    /// The spilled representation: an ordinary growable buffer.
    Heap(Vec<NodeId>),
}

impl Children {
    pub(crate) fn new() -> Self {
        Self::Inline {
            len: 0,
            ids: [Self::empty_cell(); INLINE_CAP],
        }
    }

    /// Sentinel filling unused inline cells.
    ///
    /// Sound as a "no node" marker because arena allocation refuses to ever
    /// issue slot index `u32::MAX` (see `Dom::alloc`: the fresh-slot path
    /// filters it out before the documented, unreachable-by-RAM `expect`).
    /// Since no real handle can carry that slot number, no generation value
    /// can make a live handle equal this one.
    const fn empty_cell() -> NodeId {
        NodeId::new(u32::MAX, u32::MAX)
    }

    /// The live entries as a contiguous slice, padding hidden.
    pub(crate) fn as_slice(&self) -> &[NodeId] {
        match self {
            Self::Inline { len, ids } => &ids[..usize::from(*len)],
            Self::Heap(ids) => ids,
        }
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, NodeId> {
        self.as_slice().iter()
    }

    /// Appends at the end, spilling if this crosses the inline capacity.
    pub(crate) fn push(&mut self, id: NodeId) {
        if let Self::Inline { len, ids } = self {
            let n = usize::from(*len);
            if n < INLINE_CAP {
                ids[n] = id;
                *len += 1;
                return;
            }
        }
        // Spill path: take the current contents, rebuild as a heap buffer,
        // and leave the spilled representation in place permanently.
        let taken = std::mem::replace(self, Self::Heap(Vec::new()));
        let mut buffer = match taken {
            Self::Inline { len, ids } => ids[..usize::from(len)].to_vec(),
            Self::Heap(buffer) => buffer,
        };
        buffer.push(id);
        *self = Self::Heap(buffer);
    }

    /// Inserts at `index`, shifting the entries after it right.
    ///
    /// Inserting at `len` appends, and a full inline list grows through
    /// [`Children::push`]'s spill path — this method itself never overflows
    /// the inline array.
    ///
    /// # Panics
    ///
    /// Panics if `index > len`. Callers derive the index from positions found
    /// in this same list, so out-of-range means caller-side bookkeeping broke;
    /// failing loudly beats shifting the corruption downstream.
    pub(crate) fn insert(&mut self, index: usize, id: NodeId) {
        let len = self.as_slice().len();
        assert!(index <= len, "children insert out of range");
        if index == len {
            // Appending through the dedicated path: for a full inline list
            // this performs the one-way spill conversion.
            self.push(id);
            return;
        }
        match self {
            Self::Inline { len, ids } => {
                // index < len <= INLINE_CAP holds here, so every array access
                // below is in bounds.
                ids.copy_within(index..usize::from(*len), index + 1);
                ids[index] = id;
                *len += 1;
            }
            Self::Heap(ids) => ids.insert(index, id),
        }
    }

    /// Removes and returns the entry at `index`, shifting successors left.
    ///
    /// # Panics
    ///
    /// Panics if `index >= len`, for the same reason as [`Children::insert`].
    pub(crate) fn remove_at(&mut self, index: usize) -> NodeId {
        match self {
            Self::Inline { len, ids } => {
                assert!(index < usize::from(*len), "children remove out of range");
                let removed = ids[index];
                let count = usize::from(*len);
                ids.copy_within(index + 1..count, index);
                *len -= 1;
                removed
            }
            Self::Heap(ids) => ids.remove(index),
        }
    }

    /// Position of `id` in the list, if present.
    pub(crate) fn position_of(&self, id: NodeId) -> Option<usize> {
        self.as_slice().iter().position(|&entry| entry == id)
    }

    /// Drains all entries into an owned vector, leaving this list empty.
    ///
    /// Representation is preserved: an emptied spilled list stays spilled, so
    /// churny reparent cycles cannot thrash between heap and inline storage.
    pub(crate) fn take_all(&mut self) -> Vec<NodeId> {
        let replacement = match self {
            Self::Heap(_) => Self::Heap(Vec::new()),
            Self::Inline { .. } => Self::new(),
        };
        let taken = std::mem::replace(self, replacement);
        match taken {
            Self::Inline { len, ids } => ids[..usize::from(len)].to_vec(),
            Self::Heap(ids) => ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(n: u32) -> NodeId {
        NodeId::new(n, 0)
    }

    #[test]
    fn push_stays_inline_up_to_capacity() {
        let mut children = Children::new();
        for n in 0..4 {
            children.push(handle(n));
        }
        assert!(matches!(children, Children::Inline { len: 4, .. }));
        assert_eq!(
            children.as_slice(),
            &[handle(0), handle(1), handle(2), handle(3)]
        );
    }

    #[test]
    fn push_past_capacity_spills_and_keeps_order() {
        let mut children = Children::new();
        for n in 0..6 {
            children.push(handle(n));
        }
        assert!(matches!(children, Children::Heap(_)));
        assert_eq!(
            children.as_slice(),
            &[
                handle(0),
                handle(1),
                handle(2),
                handle(3),
                handle(4),
                handle(5)
            ]
        );
    }

    #[test]
    fn spilled_list_never_returns_inline() {
        let mut children = Children::new();
        for n in 0..6 {
            children.push(handle(n));
        }
        while children.as_slice().len() > 1 {
            children.remove_at(children.as_slice().len() - 1);
        }
        assert!(matches!(children, Children::Heap(_)));
        assert_eq!(children.as_slice(), &[handle(0)]);
    }

    #[test]
    fn insert_shifts_successors() {
        let mut children = Children::new();
        children.push(handle(0));
        children.push(handle(2));
        children.insert(1, handle(1));
        assert_eq!(children.as_slice(), &[handle(0), handle(1), handle(2)]);
    }

    #[test]
    fn take_all_resets_inline_and_preserves_spilled() {
        // Inline lists reset to empty inline...
        let mut inline = Children::new();
        for n in 0..3 {
            inline.push(handle(n));
        }
        let drained = inline.take_all();
        assert_eq!(drained.len(), 3);
        assert!(matches!(inline, Children::Inline { len: 0, .. }));
        assert!(inline.as_slice().is_empty());

        // ...while spilled lists stay spilled, honoring the never-returns-
        // inline guarantee under churn.
        let mut spilled = Children::new();
        for n in 0..6 {
            spilled.push(handle(n));
        }
        let drained = spilled.take_all();
        assert_eq!(drained.len(), 6);
        assert!(matches!(spilled, Children::Heap(_)));
        assert_eq!(spilled.as_slice().len(), 0);
        spilled.push(handle(99));
        assert!(
            matches!(spilled, Children::Heap(_)),
            "refilling a drained spilled list must not re-spill from scratch"
        );
    }

    #[test]
    fn position_finds_entries_only_within_len() {
        let mut children = Children::new();
        children.push(handle(7));
        assert_eq!(children.position_of(handle(7)), Some(0));
        // padding cells beyond `len` never surface through lookups
        assert_eq!(children.position_of(NodeId::new(u32::MAX, u32::MAX)), None);
        assert_eq!(children.position_of(handle(0)), None);
    }
}
