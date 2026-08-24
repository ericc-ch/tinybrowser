# Subagent review round — 2026-08-24 (pass 3)

Two independent reviewer agents (fresh context, ruthless-mindset brief) audited
the uncommitted spec-conformance round against HEAD. A third adversarial-QA
agent died on a provider error before reporting (session
`ses_fcb8aef63ffeTAwAy7Vks2jpqe`; its unique ground — build-claim reproduction,
storm rerun — was separately reproduced by reviewer B, who re-ran tests,
clippy pedantic `-D warnings`, and fmt himself: all green).

Sessions: spec-fidelity reviewer `ses_fcb8aef65ffe7ToFtLeLISIQLF`, conventions
reviewer `ses_fcb8aef64ffe4xJDzZMRctxai6`. Findings merged and numbered below;
severity as reported. Both reviewers ran their own black-box probes against the
tree.

## Blockers

- **R3-1 — Critical, reachable panic in `:lang()`** (found independently by
  BOTH reviewers, both reproduced). `state.rs` sliced raw bytes:
  `tag[..=range.len()].eq_ignore_ascii_case(..)` panics when the index lands
  inside a multibyte character. Repro: element `lang="añx"` (or `"ab日"`),
  query `p:lang(a)` → char-boundary panic. Selector text + markup are both
  attacker-controlled; expected failure surfacing as a process crash violates
  the errors-as-values rule. Fix applied direction: whole-subtag comparison
  under extended filtering — no byte indexing anywhere.
- **R3-2 — Major, `reparent_children(div, document)` fabricates illegal
  documents.** Bulk move skips the document content model: probed to
  `Ok(())` producing three element roots, and text-under-document. The sink
  adapter wraps this method, so "trusted parser flows" does not contain the
  exposure. Fix shape: batch content-model gate when `to == document`
  (existing ∪ moved must satisfy ≤1 element, ≤1 doctype, order rules, no
  character data).

## Major

- **R3-3 — `:disabled` inheritance wrong twice** (HTML §4.15): the
  disabled-fieldset rule applies to ALL form controls (code walked ancestors
  only for option/optgroup ⇒ `<fieldset disabled><input>` answered enabled);
  option/optgroup also answer to nearest disabled `select` (not implemented).
  First-legend exemption stays a documented approximation.
- **R3-4 — `:checked` misses selectedness defaults**: first option of a
  non-multiple select with no `selected` anywhere is checked in every
  browser; statically computable, so the truth policy demands it.
- **R3-5 — `:lang()` grammar/filters incomplete**: comma-separated range
  lists (`E:lang(sr, "*-Cyrl")` is the spec's own example), wildcard
  subtags, RFC 4647 §3.3.2 extended filtering (`en-us` range matches
  `en-Latn-US`). Prefix logic itself verified off-by-one-free.
- **R3-6 — Attribute/name lookup drifted between modules**: `state::attr_value`
  ignored namespaces while `DomElement::attr_value` requires none — probed:
  SVG `<a xlink:href>` matched `:link` but not `[href]`. Same duplication for
  case-regime local-name compare and html-ns predicate. Consolidate to ONE
  lookup home (done direction: state.rs owns it; select.rs delegates).

## Minor

- **R3-7** Whitespace inside functional args (`:lang( en )`) rejected;
  Selectors 4 explicitly allows it.
- **R3-8** `:dir(up)` refusal cites Selectors 4 §dir-pseudo, which says
  other values are valid-but-no-match; current engines throw — cite engine
  behavior as the reason, not the section.
- **R3-9** `:defined ≡ true` false for unregistered hyphenated custom-element
  names (`<my-widget>` is statically known-undefined; reserved hyphenated
  names excluded). Set is fully representable — represent it.
- **R3-10** Vacuous bucket contains statically determinable subsets:
  `:default` (checked/selected attributes), `:indeterminate` (progress
  without value attr), `:in-range/:out-of-range` (fresh-page value/min/max).
  Policy violated; tests pinned the wrong story. Radio-group scoping and the
  numeric value model stay deferred with precise reasons.
- **R3-11** `:placeholder-shown` ignored placeholder-capable input types
  (checkbox with placeholder matched).
- **R3-12** read-only/write cut follows Chrome but cites §rw-pseudos as
  authority; HTML §4.16.3 / Firefox is the live counter-position — recite
  honestly.
- **R3-13** Fragment clause of the document content model (fragment with >1
  element child or a Text child into a document throws) missing from the
  gate; L14 covers splice-vs-nest but not validity. Test comment claiming
  real DOMs keep fragment nodes as children is false.
- **R3-14** `CycleForbidden` folds into HierarchyRequestError at the binding
  layer — record that mapping obligation beside the variant.
- **R3-15** Gate comments quote step numbers/text from a superseded revision
  of ensure-pre-insert-validity (algorithm now takes `childrenToExclude`;
  self-insert moved to retargeting). Keep the section anchor, drop stale
  numbers.
- **R3-16** "Refused exactly as browsers refuse unknown names" overclaims:
  `:valid`, `:invalid`, `:user-valid/-invalid`, `:paused`, `:open`, `:modal`,
  `:popover-open`, `:state(x)` are browser-known. Soften doc or absorb the
  static ones later.
- **R3-17** `lang_attr` duplicated `attr_value(.., "lang")` verbatim.
- **R3-18** Conventions nits: `is_read_write` missing rustdoc; `eq_ascii`
  named like exact-match but means ignore-case; predicate naming mixed
  (`is_*` vs `*_shown`/`*_is`).
- **R3-19** api.rs rebuilt `<body>` fixtures inline five times where
  selectors.rs extracted `body_under`; dup-attr test asserts internal Vec
  layout (defensible; get()-based preferred).
- **R3-20** `dir="auto"` deferral was justified as needing text layout —
  actually a static first-strong-character scan needing Unicode bidi
  classes. Reclassify the reason.

## Docs accuracy

- **R3-21** Findings doc claimed "fixed everything … none defects" and
  map.md echoed it; R3-1/R3-2 falsify that until fixed. Both reviewers also
  confirmed every quantitative claim (65 green, pedantic clippy clean,
  rustfmt clean) reproduces exactly.

## Held up (both reviewers, independently)

Gate core mapping exhaustively verified across mid-list insertion points
including `[comment]`/`[doctype]`/`[doctype, html]` fixtures; self-insert
no-op ordering sound; dedupe-first-wins consistent with tokenizer;
leaf-parent refusals; `children()` empty-list semantics; hyperlink set and
required/optional populations verbatim; invalid-dir inheritance fallback
correctly walks up instead of defaulting ltr; uppercase hand-built elements
behave like tokenized everywhere via the shared case regime;
InvalidState→MalformedInput correct; vacuous routing exhaustive, no wildcard
arm; state.rs judged a deep module worth keeping; CONTEXT/ADR amendments
accurate.

## Status

RESOLVED 2026-08-25. All findings landed except where noted: R3-1 via
whole-subtag positional extended filtering (regression-pinned by
`multibyte_lang_values_never_crash_the_matcher`); R3-2 via
`Dom::ensure_document_content_model` over the bulk-move result sequence
(regression-pinned by `reparent_children_into_a_document_honors_the_content_model`);
R3-3/R3-4/R3-9/R3-10/R3-11 implemented (R3-10 as documented static subsets —
default submit buttons, radio groups, and numeric ranges stay deferred with
reasons in state.rs); R3-5 grammar + RFC 4647 §3.3.2; R3-6 consolidated —
state.rs is the one home of name/attribute lookup and select.rs delegates;
R3-7 whitespace-lenient args; R3-8/R3-12/R3-14/R3-15/R3-16/R3-20 citation
and doc honesty fixes; R3-13 fragment-clause note rides with L14;
R3-17/18/19 cleanups. Compound wildcard ranges (`*-Cyrl`) require the quoted
spelling — CSS lexes an unquoted `*` separately (documented in select.rs);
the spec's own example is quoted. Verification at close: 72 workspace tests
green, clippy pedantic `-D warnings` silent, `cargo fmt --check` exit 0.
Earlier status (kept for the record): fixes were started (state.rs rewritten
toward R3-1/5/6/9/11) and then PAUSED by author request mid-surgery; the
tree did not compile until the round resumed and completed.
