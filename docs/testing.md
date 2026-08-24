# Testing Strategy

Two tiers, ordered by what they catch: parse correctness against the
spec-mandated tree, then unit/public-API tests for everything the suite
cannot see.

## Parse correctness: html5lib tree-construction suite — **landed 2026-08-25**

Vendored as a pinned submodule (`third_party/html5lib-tests`, decisions and
acceptances in
[ADR 0005](adr/0005-html5lib-tree-construction-suite.md)); the harness at
`crates/browser/tests/html5lib.rs` runs every full-document case through the
public API under both scripting-flag settings and diffs byte-exactly against
the spec-mandated tree: **3165 cases green**; fragment-context cases (192)
defer until fragment parsing exists, upstream's `<selectedcontent>` gap is a
documented divergence. This was the standing open item from the dom-layer
milestone; it covers exactly the misnesting/foster-parenting/adoption-agency
traps hand-written fixtures miss.

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

Verified at audit close: 73 tests green across the workspace, clippy pedantic
`-D warnings` clean, rustfmt clean.

## Explicitly deferred

- Full web-platform-tests corpus: needs harness machinery that belongs to
  later layers; adopt once js exists.
