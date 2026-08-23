# 05: Selector matching ships in dom v1

Type: grilling

Question: When does tree-searching (`querySelector`-style matching) get built?

Answer:

**In round one** — `dom`'s first release includes selector matching, not just parse/read/mutate.

Implementation: wire the Servo `selectors` crate (+`cssparser`, ~75 KB measured marginal, docs/size-budget.md) to our arena via `NodeId`s; expose search entry points on `Dom`/`NodeRef`. Budget impact accepted: ~2.7 MB headroom absorbs it.

Rationale: page JavaScript calls `querySelector` constantly; shipping v1 without it would force a stall the moment `js` starts consuming. Building it now keeps the whole read-side API designed once.
