# Code-review findings — dom crate audit, 2026-08-24

Full-crate review (source, tests, ADR conformance, consumer usage). Verified:
`cargo test -p dom` green (52 workspace), clippy pedantic+cargo clean, plus
black-box reproductions built against the tree.

**Status after the 2026-08-24 architecture + fix pass** (commits 158f2a6,
59502b4, a7df5b1, then 400436b…1453ec8): **every finding is resolved**
except the generation-wrap ABA note in Test gaps, which is a documented
design acceptance rather than a defect. C1 and L2 closed in the morning
architecture pass; M1–M3, L1, L3–L6 and the executable test gaps closed in
the afternoon pass.

## C1 — ✅ RESOLVED BY DESIGN: children lists collapsed to `Vec<NodeId>`

Was: confirmed panic at `children.rs:98` (`copy_within` out of bounds when
`len == INLINE_CAP == 4`, `index < len`), reachable from
`browser::parse_html` on ordinary foster-parenting markup such as
`<div><span>1</span><span>2</span><span>3</span><table>x</table></div>`.

Resolution: commit 158f2a6 deleted the dual representation entirely (ADR
0002 amended in place; `Spill` glossary term retired). Wide-list insert
coverage now lives in `api.rs::insert_into_wide_list_lands_exactly`. The
minimal spill-on-full hot-fix was considered and deliberately skipped — the
structural fix supersedes it.

## Medium

- **✅ M1 — RESOLVED (commit 400436b): `unlink_from_current_parent` follows
  the defect policy.** Divergence between the parent pointer and the child
  list now panics like every other structural site; guarded by a
  deterministic 1500-op mutation storm auditing bidirectional links after
  every step (`mutation_storm_keeps_parent_links_bidirectional`).

- **✅ M2 — RESOLVED (commit c01734c): quirks mode plumbed through.**
  `select_all` / `select_first` / `matches` take `dom::QuirksMode`
  (mirroring html5ever's three modes); full quirks applies the engine's
  WHATWG id/class case quirk, pinned by test. The js layer maps
  `Parsed.quirks_mode` when it lands.

- **✅ M3 — RESOLVED (commit 33fa6ec): state pseudo-classes parse and match
  truthfully.** `:link` matches HTML `a`/`area`/`link` with an `href`;
  `:hover`/`:active`/`:focus`/`:visited` parse but match nothing (no input,
  focus owner, or history in a headless tree); unknown pseudo-classes still
  refuse, as browsers do.

## Low

- **✅ L1 — RESOLVED with M3 (commit 33fa6ec).** `is_link` is live code again
  (it backs `:link`) and its exact-match local-name check was replaced by
  the element's case regime.
- **✅ L2 — RESOLVED (commit a7df5b1): `IllegalTarget` split into**
  `ProtectedNode` / `WrongNodeType` / `NoParent` / `SelfInsert`, keeping
  `StaleNode` + `CycleForbidden`; exact DOMException labels pinned when js
  lands.
- **✅ L3 — RESOLVED (commit 2dc44d2):** `SelectError::Syntax` carries
  structured `ParseFail { kind, message }`.
- **✅ L4 — RESOLVED with L3 (commit 2dc44d2):** the single-variant enum gave
  way to `ParseFailKind` — eight honest discriminants from empty-selector to
  malformed-input.
- **✅ L5 — RESOLVED (commit 870cea4): manifest reconciled toward ADR 0002** —
  `cssparser = "=0.34.0"` exact, matching its "exact-version partner" role;
  `selectors` stays caret (the ADR never pinned it).
- **✅ L6 — RESOLVED (commit 870cea4):** the 5381/×33 fold names itself DJB2.

## Test gaps

- **✅ Property-style coverage (commit 400436b):** the deterministic mutation
  storm audits parent-pointer/child-list duality every step without a
  property-testing dependency. (The original textbook candidate — model-
  testing `Children` against `Vec` — died with the collapse: there is no
  dual representation left to model.)
- **✅ `:nth-child(… of S)` (commit 1453ec8):** subset renumbering pinned
  against overall-position baselines.
- **⏳ Generation-wrap ABA:** accepted by design, untestable at 2^32 recycles
  per slot; the one-recycle staleness mechanism is already pinned by
  `recycled_slots_never_impersonate_dead_nodes`. Documented refusal, not an
  oversight.

## What held up

Generational scheme sound end to end (single tick at realloc verified);
sentinel-slot refusal implemented, not just claimed; structural `!Sync`
marker; exhaustive parse-error rendering; public-API-only integration suites;
stale-handle semantics proven, not asserted.
