# tinybrowser wiki

Index of project knowledge. Query starts here.

## Terms

- [CONTEXT.md](CONTEXT.md) — domain vocabulary (`NodeId`, hard seam, `Context`, conversion point, …)

## ADRs

- [0001 Workspace crates](adrs/0001-workspace-crates-with-enforced-edges.md) — history: not a monolith; live graph is 0007
- [0002 DOM layer architecture](adrs/0002-dom-layer-architecture.md) — generational arena, TreeSink in `browser` (supersedes 0003 and 0004)
- [0003 TreeSink adapter in browser](adrs/0003-treesink-adapter-in-browser.md) — superseded by 0002
- [0004 DOM v1 audit acceptances](adrs/0004-dom-v1-audit-acceptances.md) — superseded by 0002
- [0005 html5lib tree-construction suite](adrs/0005-html5lib-tree-construction-suite.md) — pinned submodule harness; fix-or-document divergences
- [0006 Net transport](adrs/0006-net-transport.md) — v1 is ureq 3 + native-tls; stealth deferred to hand-rolled btls
- [0007 Engine charter](adrs/0007-engine-charter.md) — three crates, HTML jobs + Tokio waiter, `browser` holds `Agent`

## Works

- [Engine charter](works/engine-charter/map.md) — thinner crate law and the real holes
- [Engine charter spec](works/engine-charter/spec.md) — decided engine shape (loop, crates, layers)
- [Engine charter session](works/engine-charter/session.md) — 2026-08-27 continuation

## Research

- [Size budget](researches/size-budget.md) — measured binary marginals and stack totals
- [Testing](researches/testing.md) — html5lib suite, public-API tests, live bot-gate matrix
- [WebIDL verification](researches/webidl.md) — verify-don't-generate plan against vendored IDL
- [Engine source](researches/engine-source.md) — spec then Gecko then Blink then WebKit; browse+fetch URLs; do not clone
