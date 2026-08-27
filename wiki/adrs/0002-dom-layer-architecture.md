# DOM layer architecture

`dom` is a generational arena of `NodeId` handles so JS and parser mutations never hold a Rust borrow across a reentry, and a dead handle cannot resolve to a recycled stranger. html5ever lives above it in `browser::parse_html`: storage has no tree-builder dependency (it still pins `markup5ever` for names). The sink owns parser vocabulary (chunked text). `<template>` contents are a fragment associated on `Dom`, not in the template element’s child list ([ADR 0007](0007-engine-charter.md)).

Status: accepted (supersedes [0003](0003-treesink-adapter-in-browser.md) and [0004](0004-dom-v1-audit-acceptances.md))

Nodes live in `Vec<Slot>`; public identity is `NodeId { document, slot, generation }`. The document id is unique per `Dom` so a handle cannot name a live node in a different tree. Generations tick once per reallocation; destruction empties the cell without ticking. `Dom` is `Send` and structurally `!Sync` (`PhantomData<Cell<()>>`); one worker per page, zero locks. Selector queries read `Dom`'s quirks mode (the WHATWG id/class quirk); the parser writes that flag.

The TreeSink is private in `browser`. `RefCell<Dom>` stays inside the sink: the driver is single-threaded and non-reentrant, so overlapping borrows panic instead of corrupting the tree. Adjacent `AppendText` chunks merge before they reach `dom`. `<template>` contents live on `Dom` (handle → fragment), not the element's child list and not a map on `Parsed`.

`dom` depends on `markup5ever = "=0.39.0"` (re-exported name types, pinned to html5ever) plus the Servo selector stack. `Attribute` values stay `String`, not `StrTendril`.

## Options considered

- **`Rc<RefCell>` object graph:** borrow panics scale with JS reentrancy; cycles leak.
- **`parse_html` inside `dom` / a glue crate between `dom` and `browser`:** couples storage to a parser, or adds a manifest between two already-adjacent crates.
- **Sibling links, inline/heap children, tombstones, non-generational indices:** smaller or simpler records that fail silently under adoption-agency moves or slot reuse. Inline/heap was tried and reversed after an OOB panic.
- **Hand-rolled `QualName`:** extra heap plus conversion at the sink for a zero-dep manifesto.

## Consequences

- **Fragment insertion splices** children into the parent and leaves the fragment empty, per [insert](https://dom.spec.whatwg.org/#concept-node-insert).
- **Generation-wrap ABA** after 2^32 recycles of one slot is accepted; one-recycle staleness is tested.
- **`:lang()`** reads the literal `lang` attribute only, for now. `xml:lang` is still an element attribute (`dom`). HTTP `Content-Language` is document state set by `browser` at navigation, not a `net` feature ([ADR 0007](0007-engine-charter.md)).
- **Disabled fieldset** ignores the first-legend exemption ([HTML §4.10.4](https://html.spec.whatwg.org/multipage/form-elements.html#the-fieldset-element)).
- **`:scope`** under document queries is the document element. Element-rooted scope context lands with page JS.
- **Constraint-validation and context-only pseudos** (`:valid`, `:paused`, `:open`, …) parse and match nothing until those features exist.
