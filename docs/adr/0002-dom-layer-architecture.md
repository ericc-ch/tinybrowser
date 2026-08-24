# ADR 0002: dom layer architecture

Date: 2026-08-23
Status: Accepted. Consolidates what were briefly three same-day records: the
representation ADR, the parser-placement ADR (including its name-type and
selector-dependency amendments), and the threading ticket. Reflects the
post-review mechanics — two hostile review rounds tightened several claims
before they were allowed to stand.

## Context

`dom` needs an in-memory document tree. The constraints that shaped it:

- `unsafe` is denied workspace-wide (AGENTS.md), so correctness must come from
  structure, not discipline.
- Page JavaScript will mutate the DOM reentrantly once the `js` layer exists;
  anything holding a Rust borrow across a mutation point becomes a panic farm.
- QuickJS bindings are ahead: wrapper objects need small copyable handles
  whose death is detectable, not references fighting a foreign GC.
- html5ever drives construction through `TreeSink` instructions (placement
  decided below), running HTML5 algorithms — foster parenting, the adoption
  agency — that move whole child runs between parents constantly.
- The identity hazard: nodes die, slots get recycled, and an old label must
  never resolve to a stranger ("button handle suddenly names a span").
- ADR 0001's charter row for `dom` was ambiguous about whether html5ever is a
  dependency or a function of the crate; that fork needed resolving.

## Decisions

### Representation: generational arena

- All nodes live in one flat `Vec<Slot>`; `Slot { generation: u32, node:
  Option<Node> }`. Public handle: `NodeId { slot: u32, generation: u32 }` —
  `Copy`, comparable, hashable; the only value crossing crate boundaries.
- Stale handles resolve via `Option`/`Result`, never panics. Reads distinguish
  childless (`Some(empty)`) from dead (`None`) where it matters.
- Generations tick **exactly once per change of hands, at reallocation** in
  `alloc`. Destruction empties a cell without ticking — emptiness alone kills
  handles. One site owns the policy; arithmetic wraps (`wrapping_add`), and
  the residual ABA window (billions of recycles of one slot while an outside
  handle persists) is documented as accepted by design.
- Slots are addressed by `u32`; allocation past `u32::MAX` slots is a defect
  (`expect`) — hundreds of GB of RAM away, an impossible runtime condition
  rather than an error to handle.
- Each node embeds its children as ordered IDs in a plain `Vec<NodeId>` —
  one representation, no compaction machinery (the inline/heap experiment is
  recorded under Rejected alternatives).
- Parent stored as `Option<NodeId>`; every mutation keeps the parent-pointer
  half and the child-list half in agreement, defect-panicking if they diverge.
- Links carry full generational handles, not bare slot indices. Bare indices
  would be safe-by-invariant today, but a future invariant break would fail
  silently; generations turn that failure class into a loud miss.

### Threading: Send, never Sync

- `Dom` is handed between workers (future CDP answer path) and never shared.
- Suppression of auto-derived `Sync` is **structural**: a
  `_share_forbidden: PhantomData<Cell<()>>` field (`Cell<()>` is `Send` +
  `!Sync`). Derived silence was tried implicitly and review proved `Sync`
  slipped through — hence an explicit marker whose deletion must be conscious.
- Zero locks anywhere. Rationale: one worker per page (parse → JS → report,
  sequential); QuickJS runtimes are single-threaded anyway; shared access buys
  nothing today and would tax every operation forever.

### Parser placement: html5ever lives above `dom`

- `dom` exposes representation and mutation commands only (`create_*`,
  `append`, `insert_before`, `detach`, `reparent_children`, ...). There is no
  `dom::parse_html`.
- The `html5ever::TreeSink` implementation lives above: in `browser`, or a
  thin glue crate if `browser` should stay lean (~200 lines expected).

*Resolved 2026-08-23:* hosted in `browser` as `parse_html`;
[ADR 0003](0003-treesink-adapter-in-browser.md) records the placement and
the boundary mechanics.

### Name types: pinned markup5ever re-export

- `dom` depends on exactly `markup5ever = "=0.39.0"` — name types only
  (`QualName`, `Namespace`, `LocalName`, `Prefix`), re-exported through dom's
  public API so consumers never touch markup5ever directly.
- Pinning matches html5ever 0.39 exactly, so adapter code passes parser names
  straight through as one type — no conversion layer, no duplicate `QualName`;
  element names are interned rather than copied per node.
- `Attribute` remains dom's own type (`String` value, deliberately not
  `StrTendril`) so the tokenizer's buffer type leaks nowhere.

*Amended 2026-08-23 (selector dependencies):* the "exactly one dependency"
consequence was written while selectors were still deferred. When selector
matching shipped as part of dom v1 — always placed *inside* `dom` by ADR
0001's charter row — the
manifest grew by the Servo
selector stack (`selectors`, its exact-version partner `cssparser`, plus
`precomputed-hash` for the trait dom's name wrappers must implement).
The parser-placement decision above is untouched: no parsing dependency
entered the crate. The
enforced boundary reads "no parser above, names from pinned markup5ever",
not a dependency count; the stack's size cost is measured in
`docs/size-budget.md`'s milestone section.

## Rejected alternatives

- **Sibling-link fields** (`first_child`/`next_sibling`): ~25% smaller records
  and near-parity perf at our scale, but adoption-agency/foster-parenting moves
  become multi-node pointer rewiring whose mistakes corrupt silently. Lost on
  bug-risk asymmetry.
- **Inline/heap children split** *(tried 2026-08-23, reversed 2026-08-24)*:
  four inline `NodeId`s spilling once to heap compacted small fan-out but cost
  40 in-record bytes versus 24 for a bare `Vec<NodeId>` — "compaction" made
  every record fatter — and shipped one real out-of-bounds panic before any
  measured win: `insert` into a full inline array overran its own storage,
  reachable from ordinary foster-parented table text. Children reverted to a
  plain `Vec<NodeId>`; with the split went the spill-once/thrash machinery and
  the `u32::MAX` empty-cell sentinel. Allocator traffic for small lists rises;
  binary-size impact expected ≈0 (no dependency change), to be confirmed at
  the next milestone probe.
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
  day the invariant breaks.
- **Derived-only thread stamps**: `Vec`-based fields auto-derive *both* `Send`
  and `Sync`; "silence on Sync" enforces nothing. Replaced by the marker field.
- **`parse_html` inside `dom`**: couples the storage layer to a parser forever.
- **Hand-rolled `QualName`** *(the original name-type decision, reversed)*:
  keeps manifests at literally zero dependencies but pays per-node heap strings
  for element names and forces conversion glue at the TreeSink boundary;
  purity rule lost to interning-for-free.
- **Unpinned markup5ever**: any drift from html5ever 0.39's copy forks
  `QualName` into two incompatible types.
- **Type-level container/leaf split of `Node`** *(rejected 2026-08-24, spec
  conformance pass)*: splitting children-carrying kinds (Document, Element,
  Fragment) from leaves (Text, Comment, Doctype) into distinct types would
  let the compiler prove no leaf ever gains a child list. Lost to churn:
  every arena callsite, the selector walk, and the TreeSink adapter would
  restructure for a guarantee the pre-insert validity gate already provides
  at the API level (leaves refuse children; `children()` answers an empty
  list, mirroring DOM `childNodes` being always-a-list).

## Consequences

- Records carry their child lists by value, so they stay larger than
  link-based layouts; accepted — no layout pass exists to squeeze them, RAM
  delta is trivial next to QuickJS heaps, and traversal gets children IDs for
  free in the same cache lines.
- Single-child splice is O(siblings) memmove of handles — irrelevant at real
  fan-out; bulk moves stay ordered-list operations.
- Swapping any of this later is contained: everything travels through
  `NodeId`s and the whole representation lives in one module.
- `dom`'s manifest carries the pinned name types plus the Servo selector
  stack; "no parsing dependency" is the enforced boundary, not a dependency
  count. The adapter needs no name conversion; attribute values still convert
  (`StrTendril` → `String`).
- Binary size will include markup5ever's tables at the milestone probe;
  measured against the headroom recorded in `docs/size-budget.md`.
