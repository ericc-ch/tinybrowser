# ADR 0003: The TreeSink adapter lives in `browser`

Date: 2026-08-23
Status: Accepted

## Context

ADR 0002 placed html5ever above `dom` but left the address open: "in
`browser`, or a thin glue crate if `browser` should stay lean (~200 lines
expected)". Building the adapter (commit b027315) forced three choices that
ADR left unresolved:

- Which crate hosts the sink (~330 lines including the public seam).
- `TreeSink` 0.39 hands out `&self`, while every `Dom` mutation takes
  `&mut self`; some interior mutability is unavoidable, in a workspace
  where `unsafe` is denied and ADR 0002 rejected `Rc<RefCell>` for the
  representation outright.
- html5ever delivers adjacent text as separate `NodeOrText::AppendText`
  calls; someone must own turning chunks into one text node.

## Decisions

### Placement: `browser`, not a glue crate

The line estimate was close, but the deciding argument was structural:
`browser` is already the fan-in point (ADR 0001), the one crate allowed to
hold several layers at once. A glue crate would sit between two crates that
are already legally adjacent, adding a manifest and a name for one file's
sake. The public seam is `browser::parse_html(input) -> Parsed`; the sink
type itself is private.

### Interior mutability: `RefCell<Dom>`, confined to the sink

ADR 0002 rejected `Rc<RefCell>` for the *representation*: there, borrows
would be held across JS-driven reentrancy, and panic frequency would scale
with page scripts. This boundary sits elsewhere on that axis: the driver is
single-threaded and never reenters the sink mid-call, so borrows are short,
sequential, and provably non-overlapping today; if an adapter bug ever
overlaps them, the `RefCell` panics loudly rather than letting the tree
corrupt. The `RefCell` fields stay private to the sink struct; every borrow
dies at the boundary.

### Text merges at the boundary

Adjacent `AppendText` chunks coalesce in the sink before a single mutation
reaches `dom`. Storage keeps its invariant that each command does exactly
what it says; chunking is the parser's vocabulary, not the warehouse's.

### Template contents live in a side map

Per spec, `<template>` contents sit outside the element's child list. The
sink keeps a handle-to-handle fragment map rather than modeling contents as
ordinary children, so `children()` never lies about what traversal sees.

## Rejected alternatives

- **Thin glue crate** (`parse` or similar): legal per ADR 0002, but pure
  ceremony between two already-adjacent crates. Revisit only if the sink
  grows drivers `browser` shouldn't hold.
- **Merging text inside `dom`**: taxes every caller's append path to serve
  one consumer's chunked input style.
- **`UnsafeCell` with an invariant claim**: buys nothing over `RefCell`
  here (no borrow needs to survive a call) and would spend the repo's
  zero-`unsafe` budget on a non-problem.
- **Template contents as ordinary children**: makes `children()` spec-wrong
  for exactly the elements whose contents scripts poke at.

## Consequences

- `dom` stays parser-free; `browser` gains its first real logic. Manifests
  still match ADR 0001's edge drawing.
- Parse-correctness testing (the html5lib suite) now has a seam to target:
  `browser::parse_html`.
- Size: the dom-v1 checkpoint measured this entire stack end-to-end
  (+932 KB tuned). Milestone close re-measures per the standing rule.
