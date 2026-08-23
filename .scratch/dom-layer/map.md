# dom layer

Destination: `crates/dom` v1 is shipped and size-checked — generational-arena
storage, mutation commands, and selector matching, with 45 public-API tests
green. The html5lib suite (ticket 07) moves to the TreeSink-adapter milestone,
since parse correctness needs parsing to exist (ADR 0002). Later layers (`net`,
`js`, `browser`) get their own wayfinding sessions from here.

Notes:

- Repo rules in AGENTS.md: no `unsafe`, no lint silencing, breaking changes fine when design is wrong, sub-5MB binary measured at milestones.
- Stack already decided (docs/size-budget.md): html5ever 0.39 tree builder, selectors 0.26 (~75 KB) — both now measured together as dom v1: +932 KB tuned.
- WebIDL verification strategy exists (docs/webidl.md) but is CI-side; doesn't block dom design.
- Layer-by-layer, not vertical slice. Deps go downward only; parser stays above dom — the manifest carries the pinned markup5ever name types plus the Servo selector stack, never a parsing dependency (ADR 0002).
- `js` will consume `dom` types for native bindings — dom's API is designed for that consumer.

Decisions so far:

- **First layer is dom**: bottom-up order starts at the leaf `js` needs most. (Wayfinding-session decision; its ticket record was consolidated away.)
- **Architecture** ([ADR 0002](../../docs/adr/0002-dom-layer-architecture.md)): generational arena with inline children lists, Send-not-Sync via structural marker, parser adapter above dom, pinned markup5ever name types. ✅ Shipped — commits e300d0c..961ee0c, two review rounds processed.
- [Selectors in v1](./tickets/05-selectors-in-v1.md): shipped in dom v1 via Servo's `selectors` crate — `select_all`/`select_first`/`matches` on `Dom`; entry points stayed off `NodeRef`.
- [Testing strategy](./tickets/07-testing-strategy.md): unit + public-API tests landed (45 green through public API); html5lib vendoring waits on the TreeSink adapter.
- [Size checkpoint](./tickets/08-size-checkpoint.md): answered — +932 KB tuned vs +915 KB estimated.

Not yet specified:

Out of scope:

- Layout, rendering, CSSOM cascade beyond selector matching
- Events/dispatch (decide when `js` wayfinds, if ever)
- CDP, serve/fetch bins
