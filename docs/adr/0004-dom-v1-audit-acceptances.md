# ADR 0004: dom v1 audit acceptances

Date: 2026-08-25
Status: Accepted. Consolidates the surviving open items of the dom v1 review
rounds (two full-crate passes, an adversarial two-agent re-audit (21
findings), and a follow-up diff review) into the permanent record, retiring
the scratch paper trail. Every other finding those rounds raised landed:
verified at close as 72 workspace tests green, clippy pedantic `-D warnings`
clean, rustfmt clean.

## Context

The audits ended with six findings deliberately not fixed. Each is a
documented design acceptance, a known divergence from engine behavior or a
bounded hazard, recorded with the condition that retires it, so later layers
inherit decisions instead of rediscovering them as bugs.

## Accepted limitations

### Fragment insertion nests; live-DOM insertion splices

Appending a fragment to a live parent keeps the fragment node itself as the
child. DOM insertion means the opposite: the standard's insert algorithm
promotes a `DocumentFragment` to its children before placing anything
([dom spec](https://dom.spec.whatwg.org/#concept-node-insert)). No current
caller depends on either semantics; template contents travel through the
sink's side map ([ADR 0003](0003-treesink-adapter-in-browser.md)), and the
document content model already refuses illegal fragments on the way in.
**Retires when:** the js layer implements real `appendChild`, which must
splice contents with pre-insert checks applied per spliced child; the
fragment clause of
[ensure pre-insert validity](https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity)
lands there too.

### Generation-wrap ABA

A handle survives unless its slot recycles 2^32 times while held: billions
of recycles of one slot against an outside observer. Accepted by design in
[ADR 0002](0002-dom-layer-architecture.md)'s representation section (one tick
site, wrapping arithmetic); the realistic case, one-recycle staleness, is
pinned by `recycled_slots_never_impersonate_dead_nodes`. No trigger; revisit
only on evidence.

### `:lang()` reads only the literal `lang` attribute

[Selectors 4](https://www.w3.org/TR/selectors-4/#lang-pseudo) sources the
match language from `lang`, then `xml:lang`, then inherited values, plus
HTTP `Content-Language` defaults. dom answers ancestor inheritance of `lang`
alone. **Lands with:** `net`, which owns headers and can supply
Content-Language defaults; `xml:lang` rides along.

### Disabled fieldset ignores the first-legend exemption

HTML exempts a form control from an ancestor disabled fieldset when it sits
under the fieldset's *first* `legend` element child
([HTML §4.10.4](https://html.spec.whatwg.org/multipage/form-elements.html#the-fieldset-element)).
The implementation disables option/optgroup descendants regardless of
legends. **Revisit if:** form support deepens past selector truth.

### `:scope` resolves to the document element under document queries

Matches browsers' `document.querySelectorAll(':scope')`.
[Selectors 4](https://www.w3.org/TR/selectors-4/#scope-pseudo) defines
`:scope` by the query's scoping root, so element-rooted queries answering
with the root need explicit scope-context plumbing planned for the js layer.
**Lands when:** queries take a scope context.

### Constraint-validation states parse but match nothing

`:valid`/`:invalid`/`:user-valid`/`:user-invalid` are browser-known but
statically computable only from attributes we do not yet model (validation
UI state); they currently miss vacuously per the element-state policy.
Context-dependent relatives (`:paused`, `:open`, `:modal`, `:popover-open`,
`:state()`) are vacuous by policy and stay so until their features exist.
**Lands with:** form semantics in the js layer, alongside the deferred
static subsets already named in `state.rs`.

## Consequences

- New selector/DOM divergences join this record instead of living in session
  scratch files; an acceptance without a recorded trigger is a bug waiting to
  be rediscovered.
- Each item names its landing layer, so net/js milestones inherit a checklist
  rather than archaeology.
