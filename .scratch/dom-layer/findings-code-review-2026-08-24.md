# Code-review findings — dom crate audit, 2026-08-24

Full-crate review history: pass 1 (morning) found C1/M1–M3/L1–L6; the
architecture+fix commits through `1453ec8` closed them; pass 2 (afternoon)
verified those closures and added M4–M5/L7–L13; the spec-conformance round
(working tree, 2026-08-24 evening) fixed everything in this file. Verified
at close: workspace green (65 tests), clippy pedantic `-D warnings` clean,
rustfmt clean. Black-box probes live at `/tmp/opencode/dom-probe`.

**Pass 4 (2026-08-25, in-session diff review of the conformance round):**
two fidelity holes survived pass 3 because every R3-3/R3-4 fixture kept
options as direct children of `select`. Both probed black-box, both fixed:

- **P4-1 ✅ `:checked` default-selectedness flattens `optgroup`s now.**
  `<optgroup>`-wrapped options were invisible to the first-option scan —
  a lone wrapped option answered unchecked, and an explicit pick inside a
  group let its bare sibling default-check instead. The list of options is
  every descendant option in tree order (`state::collect_descendant_options`).
  Pinned by `checked_defaults_apply_without_selected_attributes`.
- **P4-2 ✅ disabled-optgroup inheritance implemented** (§4.10.11: an
  option is disabled when its direct parent `optgroup` is). Pinned by
  `fieldset_and_select_disability_inherits`.

Structural follow-ups from the same pass: the document content model is now
encoded exactly once — `ensure_pre_insert_validity` splices the node into
the standing children at its insertion point and delegates the resulting
sequence to `ensure_document_content_model`, replacing the parallel
positional flag-scan; and the gate no longer clones `NodeKind`s (and their
attribute lists) on every insertion.

Verified after pass 4: workspace green (72 tests), clippy pedantic
`-D warnings` clean, rustfmt clean.

**Open items after the conformance round — all documented design
acceptances, none defects:**

- **L14 — Fragment insertion nests, DOM splices.** Appending a fragment to
  a live parent keeps the fragment node itself as the child; live-DOM
  insertion replaces it with its contents. No current caller depends on
  either semantics (template contents travel through the sink's side map);
  implement splicing with the js layer's real `appendChild`, where the
  content model must be checked per spliced child.
- **Generation-wrap ABA**: accepted by design (2^32 recycles per slot);
  one-recycle staleness pinned by `recycled_slots_never_impersonate_dead_nodes`.
- **`:lang()` inheritance reads only the literal `lang` attribute**;
  `xml:lang` and `Content-Language` defaults land with net.
- **Disabled-fieldset inheritance ignores the first-legend exemption**
  (option/optgroup only); revisit if form support deepens.
- **`:scope` resolves against the document element** under document-level
  queries (matching `document.qSA(':scope')` in browsers); element-base
  scoping needs the query-context wiring planned for js.

## Closed — architecture & structure

- **C1 ✅ children lists collapsed to `Vec<NodeId>`** (158f2a6): the
  confirmed panic died with the dual representation.
- **M1 ✅ unlink defect policy** (400436b): divergence panics; guarded by
  the deterministic mutation storm auditing bidirectional links each step.
- **M4 ✅ Document content model enforced**: second element child refused;
  doctype limited to documents, unique, ahead of the document element from
  both directions. Three pre-existing tests were silently building illegal
  two-root documents and were rewritten onto legal trees — independent
  confirmation the gate works.
- **M5 ✅ leaves refuse children**: Text/Comment/Doctype parents are
  `HierarchyRequest`; bulk `reparent_children` gates both endpoints on
  container kinds too; `children()` on a leaf answers an empty list
  (`childNodes` is always a list).
- **L10 ✅ duplicate attributes dedupe first-wins** in `create_element`.
- **L11 ✅ insert-before-self is a spec no-op** (`SelfInsert` deleted).
- All structural rules live in one place: `Dom::ensure_pre_insert_validity`,
  mirroring WHATWG *ensure pre-insert validity* step-for-step.

## Closed — errors

- **L2 ✅ `IllegalTarget` split** into exception-shaped variants (a7df5b1).
- **L3/L4 ✅ structured parse failures** (2dc44d2): `SelectError::Syntax`
  carries `ParseFail { kind, message }` over eight `ParseFailKind`
  discriminants; the engine-kind match has no wildcard arm.
- **L12 ✅ `InvalidState` bucketed `MalformedInput`**, no longer misread as
  user grammar misuse.
- **Taxonomy renamed**: `ProtectedNode` → `HierarchyRequest` (one variant
  per exception class, as the enum doc always claimed).

## Closed — selector fidelity

- **M2 ✅ quirks mode plumbed** (c01734c) across all three modes, quirk
  pinned by test.
- **M3/L7 ✅ state pseudo-classes match truthfully**: attribute-derived UI
  states (`:enabled/:disabled/:checked/:required/:optional/:read-only/
  :read-write/:placeholder-shown/:default/:indeterminate`),
  `:any-link` ≡ `:link`,
  `:defined` (true except unregistered hyphenated custom-element names),
  functional `:lang(ranges…)`/`:dir(direction)` via the
  engine's functional-parse hook with ancestor inheritance; `:lang()`
  takes comma-separated range lists matched under RFC 4647 extended
  filtering. Context-only states (`:hover`, `:visited`, `:focus*`,
  `:target`, `:in-range/-out-of-range`, `:autofill`) parse and miss
  vacuously. Predicates live in `crates/dom/src/state.rs`, grouped per
  spec section with citations; deferred clauses are named where they sit
  (default submit buttons, radio groups, numeric ranges).
- **L8 ✅ known pseudo-elements parse and match nothing** — empty results
  like every browser; unknown names still refuse.
- **L9 ✅ `:link`/`:any-link` match hyperlinks in any namespace** (SVG
  `<a href>` included).

## Closed — hygiene

- **L5 ✅ cssparser pinned `=0.34.0`** per ADR 0002 (870cea4).
- **L6 ✅ DJB2 named** in-place (870cea4).
- **L13 ✅ rustfmt clean**; the ~56 hunks introduced by pass 2 are gone.

## What held up

Generational scheme end to end; sentinel-slot refusal; structural `!Sync`;
exhaustive parse-error rendering; public-API-only integration suites;
quirks plumbing; storm guard; `nth-child(of S)` subset semantics; complex
`:not()`, relative `:has()`, whitespace `:empty`, fragment queries.
