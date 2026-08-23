# dom layer

Problem: tinybrowser needs its first real layer. The engine stops at DOM + JS, and every other layer (`js`, `browser`, eventually CDP) consumes DOM types — but today `crates/dom` is an empty skeleton. Building top-down or as a vertical slice would force every downstream design to guess; instead we design and ship the bottom layer first, completely.

Solution: implement `crates/dom` as a document store with pinned dependencies (`markup5ever` for interned name types, plus the Servo selector stack — never a parser): HTML-agnostic tree representation, read/mutate API built on copyable node handles, selector-based searching included. An adapter living *above* `dom` will later wire html5ever's tree construction into these APIs, keeping the parser dependency out of this crate entirely.

User stories:

1. As the TreeSink adapter author, I want plain mutation commands (`create_*`, `append`, `insert_before`, `detach`, `reparent_children`) on `Dom`, so that html5ever's instructions land in our storage without `dom` knowing parsing exists.
2. As the `js` crate author, I want small copyable `NodeId` handles whose death is detectable via `Option`/`Result`, so that QuickJS wrapper objects can hold nodes across GC cycles without borrows, leaks, or wrong-node bugs.
3. As a maintainer, I want bulk tree rearrangements to be ordered-list operations rather than pointer rewiring, so adoption-agency/foster-parenting cases fail loudly instead of silently corrupting trees.
4. As a maintainer, I want parse correctness pinned to the same test corpus production engines use, so mangled real-world markup builds exactly the tree Chrome would build.
5. As the size-budget owner, I want this milestone measured like every previous one, so regressions justify themselves in bytes against the 5 MB goal.

Implementation decisions:

- **Generational arena**: all nodes in one flat slot array; handles are `NodeId { slot: u32, gen: u32 }`. Recycled slots tick their generation, so stale handles resolve to a clean miss instead of naming a stranger. No tombstones (unbounded growth), no `Rc<RefCell>` (reentrancy panics), no bumpalo (never frees), no full SoA (complexity now, unmeasured payoff).
- **Children lists embedded per node**: each record stores its children as ordered IDs — inline up to 4 (`Inline { count: u8, ids: [u32; 4] }`), spilling once to `Heap(Vec<u32>)` past that, never returning inline. Depth never spills; only width. Bulk moves are slice ops.
- **API surface**: `Dom` (warehouse with pre-created root) exposes reads (`document`, `get -> Option<NodeRef>`, `parent`, `children -> Option<iter>` — childless and dead are distinct answers, `contains`) and mutations (`create_element/text/comment/doctype`, `append`, `insert_before`, `detach` — unlink-only, subtree survives — `reparent_children`, `set_text`, `set_comment`, `destroy`). The document root can never gain a parent or be drained. Mutation errors: `DomError { StaleNode, CycleForbidden, IllegalTarget }` *(renamed from `AttachError`, third variant added, during implementation)*; never panics on input. Elements carry markup5ever's interned `QualName` — dom re-exports it pinned to html5ever 0.39's exact version so the adapter needs no name conversion *(reversed from this ticket's original "own type" amendment; full trail in ADR 0002)*.
- **Parser placement**: no `parse_html` in `dom`. The `html5ever::TreeSink` implementation lives above (in `browser` or a thin glue crate); `dom` depends on `markup5ever = "=0.39.0"` for interned name types — later joined by the Servo selector stack, still never a parsing dependency (see ADR 0002).
- **Selectors in v1**: Servo `selectors` + `cssparser` (~75 KB measured marginal) wired to the arena through newtypes over the interned name atoms; search entry points on `Dom` only — a `NodeRef` is a pure borrowed view with no arena back-pointer (see tickets/05's completion amendment).
- **Threading contract**: `Send`, never `Sync`. The split is enforced structurally — a `_share_forbidden: PhantomData<Cell<()>>` field suppresses auto-derived `Sync`; deleting it is a conscious act. Hand-off between workers legal; simultaneous access compiler-forbidden; zero locks anywhere.
- **Node kinds**: `Document | Doctype | Element | Fragment | Text | Comment`.

Testing decisions:

- **Parse correctness bar**: vendor the html5lib tree-construction suite (data files shipped with html5ever, ~5k cases) and assert our tree matches the spec-mandated result for every case.
- **Unit tests** for what the suite can't see: stale-handle misses and generation ticks on slot reuse, inline→heap spill behavior (and no spill-back), bulk reparenting, selector matching against known trees.
- **Public-API boundary**: tests go through `Dom`'s public methods and `NodeId` handles, never private slot internals.
- **Size checkpoint at close**: throwaway probe binary genuinely exercising parse + selectors, tuned profile (`opt-level = "z"`, fat LTO, `codegen-units = 1`, stripped); marginal delta vs empty-`main` recorded in the size-budget doc alongside existing measurements.

Out of scope:

- Layout, rendering, CSSOM cascade beyond selector matching
- Events/dispatch
- Eager reclamation of detached subtrees (slots recycle lazily; revisit with GC work)
- The TreeSink adapter itself, web-platform-tests harness, WebIDL verification wiring
- CDP, `serve`/`fetch` bins

Notes:

- Repo rules apply unchanged: no `unsafe`, no lint escapes, breaking changes fine pre-consumer.
- Size context lives in `docs/size-budget.md` (stack choice, tuned-profile totals, ~2.7 MB headroom after this layer).
- Exact signatures may flex while the adapter and first consumers get written; the standing contracts are ticket-in/ticket-out handles, `Option`/`Result` on staleness, slice-op bulk moves.

Closure (2026-08-23): core storage and selectors are both landed — v1 is
complete. Two consumer-driven additions beyond the original method list:
`Dom::add_attrs_if_missing` (the TreeSink contract needs it) and the
selector trio on `Dom`. Entry points live on `Dom`, not `NodeRef` (see
ticket 05's final amendment). The html5lib suite belongs to the adapter
milestone: ADR 0002 keeps parsing above dom, so parse correctness cannot be
tested from inside this crate. dom v1 measured +932 KB tuned (ticket 08).
