# tinybrowser wiki

Index of project knowledge. Query starts here.

## Terms

- [CONTEXT.md](CONTEXT.md) — domain vocabulary (`NodeId`, hard seam, `Context`, conversion point, …)

## ADRs

- [0001 Workspace crates with enforced edges](adrs/0001-workspace-crates-with-enforced-edges.md) — downward-only crate graph; `browser` is the fan-in point
- [0002 DOM layer architecture](adrs/0002-dom-layer-architecture.md) — generational arena, TreeSink in `browser` (supersedes 0003 and 0004)
- [0003 TreeSink adapter in browser](adrs/0003-treesink-adapter-in-browser.md) — superseded by 0002
- [0004 DOM v1 audit acceptances](adrs/0004-dom-v1-audit-acceptances.md) — superseded by 0002
- [0005 html5lib tree-construction suite](adrs/0005-html5lib-tree-construction-suite.md) — pinned submodule harness; fix-or-document divergences
- [0006 Net transport](adrs/0006-net-transport.md) — v1 is ureq 3 + native-tls; stealth deferred to hand-rolled btls

## Research

- [Size budget](researches/size-budget.md) — measured binary marginals and stack totals
- [Testing](researches/testing.md) — html5lib suite, public-API tests, live bot-gate matrix
- [WebIDL verification](researches/webidl.md) — verify-don't-generate plan against vendored IDL

## Works

- [Net crate v1](works/net-crate/spec.md) — decided public API and implementation ([map](works/net-crate/map.md); tickets 01–14 done)
