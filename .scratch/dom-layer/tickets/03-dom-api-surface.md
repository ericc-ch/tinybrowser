# 03: Public API surface of `dom`

Type: grilling

Question: What types and methods form `crates/dom`'s boundary?

Answer:

**Types** (as sketched and agreed 2026-08-23):

- `NodeId { slot: u32, gen: u32 }` — Copy/Eq/Hash claim ticket; the only handle crossing crate boundaries. Stale handles resolve via `Option`/`Result`, never panics.
- `Dom` — warehouse: slot array + free list + pre-created root `document`.
- `Children` enum — `Inline { count: u8, ids: [u32; 4] } | Heap(Vec<u32>)` per ticket 02.
- `NodeKind` — `Document | Doctype | Element | Text | Comment`; element carries `QualName` and `Vec<Attribute>` (`Vec::new()` costs nothing when empty).
  *Re-amended 2026-08-23:* after first landing on a hand-rolled three-field `QualName`, the call was reversed — dom now re-exports `markup5ever`'s interned name types, pinned `=0.39.0` to match html5ever 0.39 exactly. See ADR 0003's amendment section; interning comes free and adapter glue disappears.
- `NodeRef<'_>` — borrowed read view over a `NodeId`.

**Read methods:** `document()`, `get() -> Option<NodeRef>`, `parent()`, `children()` (double-ended), `contains()`.

**Mutation methods:** `create_element`, `create_text`, `append`, `insert_before`, `detach` (unlinks, subtree stays alive — detached-but-alive is needed by JS semantics), `reparent_children`, `set_text`. Plus doctype/PI creation as demanded by the TreeSink consumer.

**Error strategy:** `DomError { StaleNode, CycleForbidden, IllegalTarget }` from mutation calls; maps later to JS `NotFoundError` / `HierarchyRequestError`. No panics on bad input. *(Renamed from `AttachError` + third variant added during implementation.)*

**Deferred by design:** eager subtree reclamation (dead subtrees recycle slots; explicit reclamation revisited with GC work), selectors (separate decision), events.

Note: exact signatures may shift when the TreeSink adapter (ticket 04) and `js` bindings get written; breaking changes are cheap pre-consumer. The contract that holds: tickets in/out, Option/Result on staleness, slice-op bulk moves.
