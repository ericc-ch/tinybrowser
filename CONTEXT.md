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

**Spill**:
The one-way move of a node's children list from inline storage (up to four IDs inside the record) to a heap buffer once exceeded.
_Avoid_: promotion, overflow

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
