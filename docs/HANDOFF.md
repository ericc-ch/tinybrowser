# Handoff (2026-09-11)

State: Branch `wpt-pass-chasing`, HEAD `2b0a24d`, tree clean. ADR 0014 steps
1-3, cross-realm option 2 (phases 1-2), and the DOM iframe lifecycle hook are
committed; iframe elements are not created yet. `cargo test --workspace` 27
suites, clippy, fmt green (all via `direnv exec .`).

Done:

- `488371c` engine owns one QuickJS `Runtime` + Tokio waiter; `Document` is a
  frame; `DOMException` inherits `Error.prototype`. Proof: 27 suites; WPT
  `dom/nodes/Node-nodeName.html`, `Element-tagName.html` passed.
- `9083c1e`, `3de769a` frame + world registries and frame-addressed
  commands/events. Proof: `realm_tests::realms_share_a_heap_and_resolve_*`
  and protocol round-trip tests.
- `bbd1f82` `DocumentStore`: trees realm-agnostic, keyed by document id.
  Proof: 27 suites green, no behavior change.
- `849b8ec` `RealmRegistry`: one wrapper per node shared by same-site realms
  with the owner realm's prototypes. Proof:
  `wrappers_are_shared_with_the_owner_realms_prototypes`.
- `c3fd899` `dom::Lifecycle` records iframe connection transitions, including
  later-inserted detached subtrees. Proof:
  `connection_transitions_record_lifecycle_events`.

In flight: nothing uncommitted. Step-4 fork open; ADR 0014 "Frame realm
creation timing" (rquickjs `Context::with` borrows the runtime; no nested
realm creation) recommends A (deferred materialization + `WindowProxy`) over
B (context pool). Resume by writing the shared `FrameTree`.

Next:

1. `FrameTree` shared by `Engine` and `Document`: mint `FrameId`, own
   `Rc<RefCell<Document>>` frames, recursive lookup for commands/events.
2. `HTMLIFrameElement` members (`contentWindow`, `contentDocument`, `srcdoc`,
   `name`) + `window.frames`/`length`; create realms at safe points.
3. `src` navigation (dial + browser mount by `FrameId`); then WebDriver
   `switch to frame`, OOPIF, `Symbol.toStringTag`, adoption, `sandbox`.

Decisions made:

- [ADR 0014](adrs/0014-frames-and-per-frame-realms.md): one `Context` per
  frame; trees realm-agnostic; wrappers shared per node, owner prototypes;
  cross-origin checks in the first iframe milestone.
- Cargo tests cover product/transport/engine only; web behavior is WPT's job.
- Realm registries live on the engine, never thread-locals.

Gotchas:

- Build only inside `direnv exec .`; targeted WPT:
  `direnv exec . ./tools/wpt/run <files>`.
- `crates/renderer/src/js/bindings.rs` `realm_tests` is the cross-realm
  canary (persistents, wrapper identity, registry lifetime).
- Prefer `World::with_document`/`with_main_document` when a guard would
  outlive the borrow.
- `Engine` drop order: frames, registry, runtime (`JS_FreeRuntime` asserts).
- WPT `exceptions.html` timeout waits on `iframe.onload`, not a hang.
