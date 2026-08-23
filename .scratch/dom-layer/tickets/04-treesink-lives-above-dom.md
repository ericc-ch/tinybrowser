# 04: html5ever TreeSink adapter lives above `dom`

Type: grilling

Question: Where does the html5ever→Dom translator live?

Answer:

**Above `dom`.** `crates/dom` ships zero dependencies: pure representation plus the public mutators from ticket 03. A thin adapter (~200 lines) implements `html5ever::TreeSink` against those APIs; it lives above — in `browser`, or its own tiny glue crate if `browser` wants to stay lean. `dom.parse_html(bytes)` does not exist.

*Clarification (2026-08-23):* the enforced boundary is **no parser dependency** — html5ever stays above dom. Dom carries exactly one dependency: `markup5ever` (name types only, pinned to html5ever 0.39's version). Full reasoning in ADR 0003 as amended.

Rationale: keeps the representation layer dependency-free and honest (it must be usable without any parser); the parser dependency is a wiring concern, not a storage concern. Cost: one indirection layer, acceptable because the trait boundary already exists anyway.

Consequence for ticket 03: signatures may flex once the adapter exists; the ticket-in/ticket-out contract stands.
