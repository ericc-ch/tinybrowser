# ADR 0002: Generational arena with inline children lists

Date: 2026-08-23
Status: Accepted

## Context

`dom` needs an in-memory document tree. The constraints that shaped it:

- `unsafe` is denied workspace-wide (AGENTS.md), so correctness must come from
  structure, not discipline.
- Page JavaScript will mutate the DOM reentrantly once the `js` layer exists;
  anything that can hold a Rust borrow across a mutation point becomes a
  panic farm.
- QuickJS bindings are ahead: wrapper objects need small copyable handles whose
  death is detectable, not references fighting a foreign GC.
- html5ever drives construction through `TreeSink` instructions (placement per
  ADR 0003), and the HTML5 algorithms it runs — foster parenting, the adoption
  agency — move whole child runs between parents constantly.
- The identity hazard: nodes die, slots get recycled, and an old label must
  never resolve to a stranger ("button handle suddenly names a span").

## Decision

- All nodes live in one flat `Vec<Slot>`; `Slot { gen: u32, node: Option<Node> }`.
- Public handle: `NodeId { slot: u32, gen: u32 }` — `Copy`, comparable,
  hashable. Generations tick on slot reuse; stale handles resolve to a clean
  miss via `Option`/`Result`, never panics. Generation arithmetic wraps
  (`wrapping_add`), which keeps it total; slot allocation refuses
  `u32::MAX`, reserving that value as an internal empty-cell sentinel.
- Each node embeds its children as ordered IDs: up to four inline
  (`Inline { len: u8, ids: [NodeId; 4] }`), spilling once to `Heap(Vec<NodeId>)`
  past that, never back. Depth never spills; width only.
- Parent stored as `Option<NodeId>`.
- Links carry full generational handles, not bare slot indices. Bare indices
  would be safe-by-invariant today (every mutation path unlinks before
  freeing), but a future invariant break would fail silently; generations turn
  that failure class into a loud miss. The ~16 bytes per record premium buys
  defense in depth.
- `destroy` recursively frees a subtree (unlink, bump generations, recycle
  slots); `detach` only unlinks. Without some destruction path the generation
  mechanism is untestable and the JS lifecycle unreachable.

## Rejected alternatives

- **Sibling-link fields** (`first_child`/`next_sibling`): ~25% smaller records
  and near-parity perf at our scale, but adoption-agency/foster-parenting moves
  become multi-node pointer rewiring whose mistakes corrupt silently. Lost on
  bug-risk asymmetry.
- **`Rc<RefCell>` object graph**: borrow-conflict panics scale with JS
  reentrancy; cycles leak; identity maps poorly onto QuickJS GC.
- **bumpalo `&'arena` refs**: individual nodes can never be freed on a live
  mutating page; lifetimes infect the js-facing API.
- **Tombstones (slots never recycled)**: unbounded growth on churny pages;
  compaction impossible once JavaScript holds IDs.
- **Full SoA field split**: complicates every callsite for gains unmeasurable
  until something runs; revisit only on evidence.
- **Non-generational indices**: reintroduces ABA reuse bugs exactly when
  external code holds stale handles.
- **Bare-slot internal links**: safe by invariant today, silently wrong the
  day the invariant breaks (see Decision).

## Consequences

- Records run ~80 bytes versus ~48 for link-based layouts; accepted — no
  layout pass exists to squeeze them, RAM delta is trivial next to QuickJS
  heaps, and traversal gets children IDs for free in the same cache lines.
- Single-child splice is O(siblings) memmove of handles — irrelevant at real
  fan-out; bulk moves are slice operations.
- Swapping strategies later is contained: everything travels through
  `NodeId`s, and inline width (4) is a constant in one module.
