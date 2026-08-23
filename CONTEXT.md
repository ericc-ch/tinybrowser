# tinybrowser Domain Terms

## Terms

**NodeId**:
Copyable handle naming exactly one live node; carries a slot number plus generation and is the only value that crosses crate boundaries.
_Avoid_: pointer, reference, node ref

**Slot**:
One cell of dom's flat node array; holds at most one node and that slot's generation counter.

**Generation**:
Counter bumped every time a slot changes contents, so recycled slots cannot impersonate dead nodes.
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
Thin layer above dom that translates html5ever's tree-construction instructions into Dom mutations; owns the parser dependency so dom stays dependency-free.
_Avoid_: parser glue, binding layer

**QualName**:
Qualified element name: namespace plus optional prefix plus local name. Comes from `markup5ever` (interned; re-exported by dom, pinned to html5ever's version).
_Avoid_: tag name (only the local part)

**Fan-in point**:
The `browser` crate — the only place allowed to depend on several layers at once (ADR 0001).
