# 02: Tree representation — generational arena with inline children lists

Type: grilling

Question: How does `crates/dom` represent the document tree in memory?

Answer:

**Committed layout** (decided 2026-08-23):

- All nodes in one flat `Vec` of slots. Handles are `NodeId { slot: u32, gen: u32 }`.
- Generational counters on recycled slots so stale handles resolve to a clean miss instead of naming a stranger (button/span reuse scenario).
- Each node record embeds its children as an ordered ID list: `Inline { count: u8, ids: [u32; 4] }`, spilling once to `Heap(Vec<u32>)` past four children; spilled never returns inline. Depth never spills; width-only.
- Parent stored as `Option<u32>`-style ID field; document order = child-list order.

**Rejected alternatives** (reasons recorded so drift has a paper trail):

- Sibling-link fields (`first_child`/`next_sibling`): smallest records (~48B vs ~64B) and near-parity perf at our scale, but adoption-agency/foster-parenting moves become silent-failure pointer rewiring across multiple nodes. Lost on bug-risk asymmetry.
- `Rc<RefCell>` object graph: borrow-conflict panics scale with JS reentrancy; cycles leak; identity maps poorly onto QuickJS GC.
- bumpalo `&'arena` refs: never frees individual nodes (live DOM mutates forever); lifetimes infect the js-facing API.
- Tombstones (never recycle slots): unbounded memory growth on churny pages; compaction impossible once JS holds IDs.
- Full SoA split of node fields: complicates every callsite for wins unmeasurable until something runs; re-evaluate only if measurements demand.
- Homegrown non-generational indices: reintroduces ABA reuse bugs exactly when JS holds stale handles.

**Rationale anchors:** correctness must be structural (no unsafe, no lint escapes — AGENTS.md); JS-driven reentrant mutation is a permanent condition; bulk reparenting should be slice ops; IDs are plain ints so the future QuickJS binding layer holds copyable, death-detectable handles. Inline width N=4 is a tuning knob owned by one enum; swapping layouts later is contained because everything travels through `NodeId`.

Related context: html5ever remains the tree-construction driver (see size-budget decision); it shouts instructions at a `TreeSink` we implement over this arena. rcdom was measured only as a stand-in sink, not adopted.
