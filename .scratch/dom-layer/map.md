# dom layer

Destination: `crates/dom` v1 is shipped and size-checked — generational-arena
storage, mutation commands, and selector matching, with 45 public-API tests
green. The html5lib suite (ticket 07) moves to the TreeSink-adapter milestone,
since parse correctness needs parsing to exist (ADR 0003). Later layers (`net`,
`js`, `browser`) get their own wayfinding sessions from here.

Notes:

- Repo rules in AGENTS.md: no `unsafe`, no lint silencing, breaking changes fine when design is wrong, sub-5MB binary measured at milestones.
- Stack already decided (docs/size-budget.md): html5ever 0.39 tree builder, selectors 0.26 (~75 KB) — both now measured together as dom v1: +932 KB tuned.
- WebIDL verification strategy exists (docs/webidl.md) but is CI-side; doesn't block dom design.
- Layer-by-layer, not vertical slice. Deps go downward only; parser stays above dom.
- `js` will consume `dom` types for native bindings — dom's API is designed for that consumer.

Decisions so far:

- [First layer is dom](./tickets/01-dom-is-first-layer.md): bottom-up order starts at the leaf `js` needs most.
- [Tree representation](./tickets/02-tree-representation.md): generational arena, flat slot array, children as ordered ID lists (≤4 inline, spill once to heap).
- [dom API surface](./tickets/03-dom-api-surface.md): NodeId tickets, Dom warehouse, read/mutation method set, Option/Result error strategy.
- [TreeSink above dom](./tickets/04-treesink-lives-above-dom.md): dom carries no parser; html5ever adapter lives higher up.
- [Selectors in v1](./tickets/05-selectors-in-v1.md): shipped in dom v1 — `select_all`/`select_first`/`matches` on `Dom`; entry points stayed off `NodeRef`.
- [Send, not Sync](./tickets/06-send-not-sync.md): hand-off between workers legal; concurrent access compiler-forbidden; zero locks.
- [Testing strategy](./tickets/07-testing-strategy.md): unit + public-API tests landed; html5lib vendoring deferred with the adapter.
- [Size checkpoint](./tickets/08-size-checkpoint.md): answered — +932 KB tuned vs +915 KB estimated.

Not yet specified:

Out of scope:

- Layout, rendering, CSSOM cascade beyond selector matching
- Events/dispatch (decide when `js` wayfinds, if ever)
- CDP, serve/fetch bins
