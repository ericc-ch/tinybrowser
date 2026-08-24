# Testing Strategy

Two tiers, ordered by what they catch: parse correctness against the
spec-mandated tree, then unit/public-API tests for everything the suite
cannot see.

## Parse correctness: html5lib tree-construction suite — **open**

Vendor the suite (~5k cases, shipped as data files in html5ever's repo) and
run every case through `browser::parse_html`
([ADR 0003](adr/0003-treesink-adapter-in-browser.md)), asserting the
spec-mandated tree. This is the same bar production engines use and covers
exactly the misnesting/foster-parenting/adoption-agency traps hand-written
fixtures miss.

This is the standing open item from the dom-layer milestone; the unit tier
below is landed around it.

## Unit and public-API tests — landed

- Arena mechanics: stale `NodeId` → clean miss, generation ticks on slot
  reuse, recycled slots never impersonate dead nodes.
- Content-model gates: pre-insert validity per WHATWG rule anchors, bulk
  moves (`reparent_children`) into documents answering to the same model.
- Selector matching over known trees, including state pseudo-class fidelity
  suites (`:checked`/`:disabled` inheritance, `:lang()` ranges, quirks-mode
  case regimes).
- Mutation storms auditing bidirectional link invariants at every step.
- Integration suites go through the public API only — no arena internals.

Verified at audit close: 72 tests green across the workspace, clippy pedantic
`-D warnings` clean, rustfmt clean.

## Explicitly deferred

- Full web-platform-tests corpus: needs harness machinery that belongs to
  later layers; adopt once js exists.
