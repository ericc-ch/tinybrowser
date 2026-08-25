# 01: Destination and constraints

Type: grilling

Question: What does the net-crate effort deliver, which consumers pin its
API, and how hard is the stealth seam?

Answer:

- **Full build plan** (option b): a decided type-level API contract for
  `net` v1 *plus* implementation sequencing with size checkpoints — not
  requirements-only, not API-only.
- **Consumers that decide the shape**: `browser`'s navigation lifecycle,
  and the JS-world surfaces **fetch, XHR, WebSocket** (EventSource
  excluded). Anything all three can share is the contract; anything only
  one needs sits behind that consumer's adapter, not in `net`'s core.
- **Hard stealth seam** (option a): `net` owns every public type; no `ureq`
  type crosses its boundary ever. Rationale: ADR 0006 defers canonical-Chrome
  wire behavior to a hand-rolled-btls milestone; that swap must be internal
  to `net`, invisible to `browser` and `js`.
