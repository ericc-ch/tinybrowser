# tinybrowser Domain Terms

## Terms

**NodeId**:
Copyable handle naming exactly one live node; carries a slot number plus generation and is the only value that crosses crate boundaries.
_Avoid_: pointer, reference, node ref

**Slot**:
One cell of dom's flat node array; holds at most one node and that slot's generation counter.

**Generation**:
Counter bumped when a freed slot is reallocated, so recycled slots cannot impersonate dead nodes.
_Avoid_: version, epoch

**Stale handle**:
A NodeId whose node was destroyed; every lookup reports absence instead of returning some other node.
_Avoid_: dangling reference

**Quirks mode**:
The document-compatibility mode html5ever reports for a parsed page (NoQuirks / LimitedQuirks / Quirks); dom's selector queries take it because full quirks makes class/id matching ASCII-case-insensitive.
_Avoid_: compatibility mode, IE mode

**Detach**:
Unlinking a subtree from its parent while keeping every node alive.
_Avoid_: remove (ambiguous with destroy)

**Destroy**:
Recursively freeing a subtree's slots so every handle into it goes stale.
_Avoid_: drop, delete

**TreeSink adapter**:
Thin layer above dom that translates html5ever's tree-construction instructions into Dom mutations; lives in `browser` as `browser::parse_html` (ADR 0003) and owns the parser dependency, keeping dom free of parser crates (dom itself carries only markup5ever for names).
_Avoid_: parser glue, binding layer

**QualName**:
Qualified element name: namespace plus optional prefix plus local name. Comes from `markup5ever` (interned; re-exported by dom, pinned to html5ever's version).
_Avoid_: tag name (only the local part)

**Fan-in point**:
The `browser` crate — the only place allowed to depend on several layers at once (ADR 0001).

**Scope**:
The live node a selector query is rooted at; candidates are its descendants in document order, and the scope itself is never one of its own results — while matching still sees real ancestors above it.
_Avoid_: root (means the document's root element), context

**Pre-insert validity**:
The one gate every dom insertion walks (`append`, `insert_before`), mirroring WHATWG's ensure-pre-insert algorithm by rule (anchors, not step numbers): container-kind parents only, document content model (one element child, doctype first), cycle refusal. Bulk moves (`reparent_children`) into a document answer to the same model over the resulting sequence. Refusals are `DomError::HierarchyRequest` / `CycleForbidden`.
_Avoid_: append checks, validation scattered per method

**Element state**:
A pseudo-class truth (`:disabled`, `:lang(en)`, …) answered by `state.rs` against static markup — fully when markup determines it, otherwise as a documented static subset; states whose context cannot exist in a headless tree (pointer, focus, history) parse but match nothing.
_Avoid_: pseudo-class handling (that word covers parsing too)

**HTML integration point**:
A foreign-content element where HTML parsing resumes instead of breaking out: SVG `foreignObject`/`desc`/`title` (unconditional), plus MathML `annotation-xml` when its `encoding` says HTML — that last one is answered by our TreeSink from a flag recorded at element creation (`browser`'s `integration_points`), per [html.spec.whatwg.org §13.2.6.6](https://html.spec.whatwg.org/multipage/parsing.html#html-integration-point).
_Avoid_: integration element, breakout point
