# dom layer

Destination: `crates/dom` v1 shipped and size-checked (+932 KB tuned); the
TreeSink adapter landed in `browser` — `parse_html` end-to-end, commit
b027315, decisions in ADR 0003. Remaining in this session's scope: the
html5lib tree-construction suite against `browser::parse_html`. Later
layers (`net`, `js`) get their own wayfinding sessions from here.

Notes:

- **Audit closed**: all findings from [findings-code-review-2026-08-24.md](./findings-code-review-2026-08-24.md) are resolved (C1/L2 via the collapse + error split; M1–M3, L1, L3–L6 via the follow-up pass; only the generation-wrap ABA acceptance remains as documented design intent). The dom layer is clear for the html5lib suite work.
- Repo rules in AGENTS.md: no `unsafe`, no lint silencing, breaking changes fine when design is wrong, sub-5MB binary measured at milestones.
- Stack decided (docs/size-budget.md): html5ever 0.39 tree builder plus the Servo selector stack, measured together at dom v1 close — +932 KB tuned, ~2.58 MB headroom of 5 MB.
- WebIDL verification strategy lives in docs/webidl.md; CI-side, independent of dom design.
- Layer-by-layer, not vertical slice. Deps go downward only; parsing stays above dom (ADR 0001 charter rows as amended, ADR 0002).
- `js` will consume `dom` types for native bindings — dom's API is designed for that consumer.

Decisions so far:

- **First layer is dom**: bottom-up order starts at the leaf `js` needs most.
- **Architecture** ([ADR 0002](../../docs/adr/0002-dom-layer-architecture.md)): generational arena with inline children lists, Send-not-Sync via structural marker, parser adapter above dom, pinned markup5ever name types. ✅ Shipped through two review rounds (commits e300d0c..961ee0c).
- **Selectors shipped in dom v1**: Servo `selectors` + cssparser over interned name atoms; `select_all`/`select_first`/`matches` on `Dom`; a `NodeRef` stays a pure borrowed view.
- **Testing** ([ticket 07](./tickets/07-testing-strategy.md)): unit + public-API tests landed (52 green across the workspace); parse correctness runs against `browser::parse_html` via the html5lib tree-construction suite — the one open item.
- **Size checkpoint answered**: +932 KB tuned vs +915 KB estimated, recorded in [docs/size-budget.md](../../docs/size-budget.md)'s milestone section.
- **TreeSink adapter** ([ADR 0003](../../docs/adr/0003-treesink-adapter-in-browser.md)): hosted in `browser`, boundary-only `RefCell`, text merging at the seam, template contents in a side map.

Out of scope:

- Layout, rendering, CSSOM cascade beyond selector matching
- Events/dispatch (decide when `js` wayfinds, if ever)
- CDP, serve/fetch bins
