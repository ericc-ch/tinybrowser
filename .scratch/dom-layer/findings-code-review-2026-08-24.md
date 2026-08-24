# Code-review findings — dom crate audit, 2026-08-24

Full-crate review (source, tests, ADR conformance, consumer usage). Verified:
`cargo test -p dom` green (52 workspace), clippy pedantic+cargo clean, plus
black-box reproductions built against the tree.

**Status after the 2026-08-24 architecture pass** (commits 158f2a6, 59502b4,
a7df5b1): C1 fixed structurally (children lists are a plain `Vec<NodeId>`;
the buggy seam no longer exists — hot-fix skipped by decision) and L2 fixed
(`IllegalTarget` split). Everything below marked ⏳ is still open. M1 was
left as-is during the collapse to keep that diff reviewable; it still needs
a policy decision.

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

- **⏳ M1 — `unlink_from_current_parent` breaks the failure policy**
  (`arena.rs:520-527`). Every other site defect-panics on parent-pointer/
  child-list divergence ("panicking beats reporting a lying stale node");
  this one silently continues. Consequence is not benign: `append` would then
  push the child into the new parent's list while it still sits in the old
  one — one node, two parents, silent. Pick one policy; the rest of the crate
  argues for the panic.

- **⏳ M2 — Quirks mode dropped between layers.** `browser` tracks
  `Parsed.quirks_mode`; `select.rs` hardcodes `QuirksMode::NoQuirks` twice
  (`find_matches`, `matches`) with no API to pass it through. In quirks mode
  browsers flip attribute-value case handling for a known HTML attribute
  list. Either plumb it through `select_all`/`select_first`/`matches` or
  document the v1 refusal; today `Parsed.quirks_mode` answers a question the
  selector engine cannot ask.

- **⏳ M3 — State pseudo-classes throw; browsers do not.** `:hover`, `:focus`,
  `:link` fail at parse time with `SelectError::Syntax` (verified). Real
  `querySelectorAll(":link")` never throws — it matches (vacuously or not).
  The module doc's "as in browsers" claim holds for pseudo-elements only.
  Acceptable v1 cut if documented; undocumented, it becomes a JS-layer
  surprise (script gets an exception instead of an empty result).

## Low

- **L1 — `is_link` is unreachable dead code** (`select.rs:527-532`):
  `PseudoClass` is uninhabited and `:link` never parses, so nothing can call
  it. Latent case-sensitivity inconsistency inside (`matches!("a"|"area"|"link")`
  is exact-match, contradicting the normalize-hand-built-names policy in
  `has_local_name`). Delete or fix when state pseudo-classes land.
- **✅ L2 — RESOLVED (commit a7df5b1): `IllegalTarget` split into**
  `ProtectedNode` / `WrongNodeType` / `NoParent` / `SelfInsert`, keeping
  `StaleNode` + `CycleForbidden`; exact DOMException labels pinned when js
  lands.
- **⏳ L3 — `SelectError::Syntax(String)` is stringly typed**: loses structure
  exactly where DOMException mapping will need it. Carry `ParseFail` or a
  static discriminant; keep the `Display` rendering.
- **⏳ L4 — `ParseFail` is a single-variant enum** wrapping
  `SelectorParseErrorKind` — ceremony with no behavior. Give it fields or
  drop it.
- **⏳ L5 — Pin drift vs ADR wording**: `markup5ever` is `=0.39.0` but
  `selectors = "0.26"` / `cssparser = "0.34"` are caret ranges, while ADR
  0002 calls cssparser the "exact-version partner". Reconcile manifest or ADR.
- **⏳ L6 — `AttrValue::precomputed_hash`** hand-rolls DJB2 (5381, ×33) with no
  comment naming it. Functionally fine; name the algorithm.

## Test gaps

- No property/model tests anywhere; `Children` vs `Vec<NodeId>` is the
  textbook candidate and would have caught C1 immediately.
- `parse_nth_child_of()` enabled (`select.rs:224-226`), zero tests exercise
  `:nth-child(… of S)`.
- Generation-wrap ABA acceptance is documented but not pinned by any
  executable test.

## What held up

Generational scheme sound end to end (single tick at realloc verified);
sentinel-slot refusal implemented, not just claimed; structural `!Sync`
marker; exhaustive parse-error rendering; public-API-only integration suites;
stale-handle semantics proven, not asserted.
