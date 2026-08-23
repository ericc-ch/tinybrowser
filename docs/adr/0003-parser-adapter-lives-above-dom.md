# ADR 0003: Parser adapter lives above dom

Date: 2026-08-23
Status: Accepted (amends the `dom` charter row of ADR 0001;
decision 2 amended same day — see Amended below; decision-consequence
"exactly one dependency" amended 2026-08-23 when selectors shipped —
see second Amended section)

## Context

ADR 0001's charter table described `dom` as "html5ever parse → arena tree,
selectors" while listing its dependencies as "—" — ambiguous about whether
html5ever is a dependency or merely a consumer-facing function of the crate.
Wayfinding for the dom layer (`.scratch/dom-layer/`) had to resolve it, and hit
a second, smaller fork: which qualified-name type elements carry.

## Decision

1. **html5ever stays above `dom`.** `dom` exposes only representation and
   mutation commands (`create_*`, `append`, `insert_before`, `detach`,
   `reparent_children`, ...). The `html5ever::TreeSink` implementation lives
   above: in `browser`, or a thin glue crate if `browser` should stay lean.
   There is no `dom::parse_html`.
2. **`dom` depends on `markup5ever = "=0.39.0"` for name types only**
   (`QualName`, `Namespace`, `LocalName`, `Prefix`), re-exported through dom's
   public API so consumers never touch markup5ever directly. The version is
   pinned to exactly what html5ever 0.39 depends on, so adapter code passes
   parser names straight through as one type — no conversion layer, no
   duplicate `QualName`. Element names are interned rather than copied per
   node.

### Amended 2026-08-23 (name types)

Originally decision 2 went the other way: a hand-rolled three-field
`QualName` to keep `dom` at literally zero dependencies, with conversion glue
at the adapter boundary. Reversed after implementation once the trade was
stated plainly: interning for free, zero glue, one vocabulary shared with the
parser ecosystem — versus a purity rule whose only content was the dependency
count. The parser boundary (decision 1) is unchanged and remains the point of
the ADR.

### Amended 2026-08-23 (selector dependencies)

The consequence line "dom's manifest carries exactly one dependency" was
written while selectors were still deferred (ticket 05, first amendment).
When selector matching shipped as part of dom v1 — per ADR 0001's charter
row and the dom-layer spec, which always placed it *inside* `dom` — the
manifest grew by the Servo selector stack (`selectors`, its exact-version
partner `cssparser`, plus `precomputed-hash` for the trait dom's name
wrappers must implement). Decision 1 is untouched: no parsing dependency
entered the crate. The enforced boundary reads "no parser above, names from
pinned markup5ever", not a dependency count; the size cost of the stack is
measured in `docs/size-budget.md`'s milestone section.

## Rejected alternatives

- **`parse_html` inside `dom`**: couples the storage layer to a parser forever
  and makes ADR 0001's "depends: —" a lie in spirit.
- **Hand-rolled `QualName`** *(the original decision 2)*: keeps manifests at
  zero but pays per-node heap strings for every element name and forces
  conversion glue at the TreeSink boundary; rejected on revision.
- **Unpinned markup5ever**: any drift between dom's markup5ever and
  html5ever 0.39's would fork `QualName` into two incompatible types — hence
  the exact-version pin.

## Consequences

- `dom`'s manifest carries the pinned name types plus the selector stack;
  "no parsing dependency" is the enforced boundary, not a dependency count
  (see second Amended section).
- The adapter needs no name conversion; attribute values still convert
  (`StrTendril` → `String`) because dom defines its own `Attribute`.
- Binary size will include markup5ever's tables at the milestone probe;
  accepted against the headroom recorded in `docs/size-budget.md`.
