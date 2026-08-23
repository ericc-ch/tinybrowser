# 05: Selector matching ships in dom v1

Type: grilling

Question: When does tree-searching (`querySelector`-style matching) get built?

Answer:

**In round one** — `dom`'s first release includes selector matching, not just parse/read/mutate.

Implementation: wire the Servo `selectors` crate (+`cssparser`, ~75 KB measured marginal, docs/size-budget.md) to our arena via `NodeId`s; expose search entry points on `Dom`/`NodeRef`. Budget impact accepted: ~2.7 MB headroom absorbs it.

Rationale: page JavaScript calls `querySelector` constantly; shipping v1 without it would force a stall the moment `js` starts consuming. Building it now keeps the whole read-side API designed once.

---

*Amended 2026-08-23, during implementation:* selectors slipped out of the core-storage milestone — the arena needed review-hardening first (two review rounds found real defects), and selector matching wants the read-side API (`NodeRef`, child iteration) stable underneath it. **Deferred to the immediate next milestone, still before `js` consumes anything.** This consciously supersedes "round one" above; the spec's v1-closure checklist should be read as core-storage + selectors, with only the former landed so far.

---

*Amended 2026-08-23, at completion:* shipped in `crates/dom/src/select.rs`. Servo `selectors` 0.26 + `cssparser` 0.34 over newtypes that wrap the same interned `markup5ever` atoms elements carry, so compiled selectors compare names without materializing strings. Entry points: `Dom::select_all` / `Dom::select_first` / `Dom::matches` (`querySelectorAll` / `querySelector` / `Element.matches` semantics; candidates are descendants of the scope, scope itself excluded, but matching sees real ancestors). Structural features on (`:is`, `:where`, `:has`, `nth-of`); pseudo-classes and pseudo-elements refused at parse time — no node state or generated nodes exist to match. One deviation from this ticket's wording: search entry points live on `Dom` only, not on `NodeRef` — a `NodeRef` is a pure borrowed view with no arena back-pointer, and adding one would have re-opened the twice-reviewed core type for syntax sugar. Size measured per ticket 08.
