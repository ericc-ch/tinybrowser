# dom layer

Destination: `crates/dom` v1 shipped and size-checked (+932 KB tuned); the
TreeSink adapter landed in `browser` — `parse_html` end-to-end, commit
b027315, decisions in ADR 0003. Remaining in this session's scope: the
html5lib tree-construction suite against `browser::parse_html`. Later
layers (`net`, `js`) get their own wayfinding sessions from here.

Notes:

- **Audit closed**: both review passes plus the 2026-08-24 evening conformance round are resolved — final state in [findings-code-review-2026-08-24.md](./findings-code-review-2026-08-24.md). Remaining acceptances are documented there (fragment-splice divergence L14, `:scope` element-base wiring, generation-wrap ABA); they land with or before the js layer. html5lib tree-construction suite is the open work item.
- **Pass 3 (subagent review)**: [findings-subagent-review-2026-08-24.md](./findings-subagent-review-2026-08-24.md) — two independent agents re-audited the conformance round and found a reachable `:lang()` panic plus a bulk-move content-model hole among 21 findings; all accepted findings landed the same week (72 tests green, pedantic clippy and rustfmt clean).
- **Pass 4 (diff review, 2026-08-25)**: two optgroup fidelity holes survived pass 3's fixtures — `:checked` default-selectedness ignored wrapped options, and disabled-optgroup inheritance was unimplemented. Both fixed and pinned; the document content model is now encoded exactly once (pre-insert delegates its resulting sequence to the same checker bulk moves use). See the Pass 4 section of [findings-code-review-2026-08-24.md](./findings-code-review-2026-08-24.md).
- Repo rules in AGENTS.md: no `unsafe`, no lint silencing, breaking changes fine when design is wrong, sub-5MB binary measured at milestones.
- Stack decided (docs/size-budget.md): html5ever 0.39 tree builder plus the Servo selector stack, measured together at dom v1 close — +932 KB tuned, ~2.58 MB headroom of 5 MB.
- WebIDL verification strategy lives in docs/webidl.md; CI-side, independent of dom design.
- Layer-by-layer, not vertical slice. Deps go downward only; parsing stays above dom (ADR 0001 charter rows as amended, ADR 0002).
- `js` will consume `dom` types for native bindings — dom's API is designed for that consumer.

Decisions so far:

- **First layer is dom**: bottom-up order starts at the leaf `js` needs most.
- **Architecture** ([ADR 0002](../../docs/adr/0002-dom-layer-architecture.md)): generational arena with inline children lists, Send-not-Sync via structural marker, parser adapter above dom, pinned markup5ever name types. ✅ Shipped through two review rounds (commits e300d0c..961ee0c).
- **Selectors shipped in dom v1**: Servo `selectors` + cssparser over interned name atoms; `select_all`/`select_first`/`matches` on `Dom`; a `NodeRef` stays a pure borrowed view.
- **Testing** ([ticket 07](./tickets/07-testing-strategy.md)): the unit/public-API half has landed — 65 green across the workspace, now including content-model refusals and the state-pseudo-class fidelity suites. Parse correctness against `browser::parse_html` via the html5lib tree-construction suite remains the one open item this ticket tracks.
- **Size checkpoint answered**: +932 KB tuned vs +915 KB estimated, recorded in [docs/size-budget.md](../../docs/size-budget.md)'s milestone section.
- **TreeSink adapter** ([ADR 0003](../../docs/adr/0003-treesink-adapter-in-browser.md)): hosted in `browser`, boundary-only `RefCell`, text merging at the seam, template contents in a side map.

Out of scope:

- Layout, rendering, CSSOM cascade beyond selector matching
- Events/dispatch (decide when `js` wayfinds, if ever)
- CDP, serve/fetch bins
