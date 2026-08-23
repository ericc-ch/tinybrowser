# dom layer

Problem: tinybrowser needs its first real layer. The engine stops at DOM + JS, and every other layer (`js`, `browser`, eventually CDP) consumes DOM types — but today `crates/dom` is an empty skeleton. Building top-down or as a vertical slice would force every downstream design to guess; instead we design and ship the bottom layer first, completely.

Solution: implement `crates/dom` as a document store with a single, pinned dependency (`markup5ever`, name types only): HTML-agnostic tree representation, read/mutate API built on copyable node handles, selector-based searching included. An adapter living *above* `dom` will later wire html5ever's tree construction into these APIs, keeping the parser dependency out of this crate entirely.

User stories:

1. As the TreeSink adapter author, I want plain mutation commands (`create_*`, `append`, `insert_before`, `detach`, `reparent_children`) on `Dom`, so that html5ever's instructions land in our storage without `dom` knowing parsing exists.
2. As the `js` crate author, I want small copyable `NodeId` handles whose death is detectable via `Option`/`Result`, so that QuickJS wrapper objects can hold nodes across GC cycles without borrows, leaks, or wrong-node bugs.
3. As a maintainer, I want bulk tree rearrangements to be ordered-list operations rather than pointer rewiring, so adoption-agency/foster-parenting cases fail loudly instead of silently corrupting trees.
4. As a maintainer, I want parse correctness pinned to the same test corpus production engines use, so mangled real-world markup builds exactly the tree Chrome would build.
5. As the size-budget owner, I want this milestone measured like every previous one, so regressions justify themselves in bytes against the 5 MB goal.

Implementation decisions:

- **Generational arena**: all nodes in one flat slot array; handles are `NodeId { slot: u32, gen: u32 }`. Recycled slots tick their generation, so stale handles resolve to a clean miss instead of naming a stranger. No tombstones (unbounded growth), no `Rc<RefCell>` (reentrancy panics), no bumpalo (never frees), no full SoA (complexity now, unmeasured payoff).
- **Children lists embedded per node**: each record stores its children as ordered IDs — inline up to 4 (`Inline { count: u8, ids: [u32; 4] }`), spilling once to `Heap(Vec<u32>)` past that, never returning inline. Depth never spills; only width. Bulk moves are slice ops.
- **API surface**: `Dom` (warehouse with pre-created root) exposes reads (`document`, `get -> Option<NodeRef>`, `parent`, `children -> Option<iter>` — childless and dead are distinct answers, `contains`) and mutations (`create_element/text/comment/doctype`, `append`, `insert_before`, `detach` — unlink-only, subtree survives — `reparent_children`, `set_text`, `set_comment`, `destroy`). The document root can never gain a parent or be drained. Mutation errors: `DomError { StaleNode, CycleForbidden, IllegalTarget }`; never panics on input. Elements carry markup5ever's interned `QualName` — dom re-exports it pinned to html5ever 0.39's exact version so the adapter needs no name conversion *(reversed from this ticket's original "own type" amendment; full trail in ADR 0003)*.
- **Parser placement**: no `parse_html` in `dom`. The `html5ever::TreeSink` implementation lives above (in `browser` or a thin glue crate); `dom` carries exactly one dependency — `markup5ever = "=0.39.0"` for interned name types (amended; see ADR 0003).
- **Selectors in v1**: wire Servo `selectors` + `cssparser` (~75 KB measured marginal) to the arena through `NodeId`s; search entry points on `Dom`/`NodeRef`.
- **Threading contract**: `Send` only (auto-derived), not `Sync`. Hand-off between workers legal; simultaneous access compiler-forbidden; zero locks anywhere.
- **Node kinds**: `Document | Doctype | Element | Text | Comment`.

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
