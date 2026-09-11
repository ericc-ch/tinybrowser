# Handoff (2026-09-11)

State: Branch `wpt-pass-chasing`, HEAD `849b8ec`. ADR 0014 steps 1-3 and the
cross-realm foundation (long-term option 2, phases 1-2) are committed and
green (`cargo test --workspace` 27 suites, clippy, fmt). iframe elements
themselves are not created yet; the cross-realm base they need now works.

Done (this session, each commit verified):

- `488371c` `renderer::Engine` hosts one QuickJS `Runtime` and one Tokio
  waiter per renderer process; `Document` is one frame sharing both.
  `DOMException` inherits `Error.prototype`. Browser-conformance cargo
  stand-ins were deleted; WPT is the gate.
- `9083c1e` engine frame registry + `FrameId`; the JS world moved from runtime
  userdata (one slot per process) to a per-context registry; unit test proves
  two realms share one heap and resolve their own world.
- `3de769a` frame-addressed commands/events (`Command::Mount`/`Eval`/... and
  `FromRenderer::Event` carry `FrameId`); renderer-link subscribers receive
  `(FrameId, TabEvent)`; `TabError::UnknownFrame`.
- `bbd1f82` **realm-agnostic trees**: `DocumentStore` maps globally unique
  document ids to `Parsed` trees, shared by every frame. `World` keeps only
  its owned document ids plus realm state.
- `849b8ec` **shared wrappers**: `RealmRegistry` (owned by the engine) maps
  document ids to their owning realm and caches one wrapper per node for the
  whole process, created with the owner realm's prototypes. Test
  `wrappers_are_shared_with_the_owner_realms_prototypes` proves identity,
  prototypes, cross-realm reads and mutations. Registry lifetime is tied to
  the engine so cached values never outlive the QuickJS heap.

Next (ADR 0014 step 4):

1. **DOM lifecycle hooks.** Add insertion/post-connection/removing steps to
   `dom` (iframe creates its content navigable in post-connection and destroys
   it in removing). Parser insertion can materialize a child realm
   synchronously; a scripted `appendChild(iframe)` cannot, because rquickjs
   cannot create a context inside a running one (see ADR 0014 "Frame realm
   creation timing"). Frame trees (`FrameTree` shared by engine and documents)
   should be the next structural step, then iframe bindings with a
   `WindowProxy` that binds to the child realm once it exists.
2. **iframe bindings + srcdoc.** `HTMLIFrameElement` members (`contentWindow`,
   `contentDocument`, `srcdoc`, `name`); filter creation must reach the engine:
   `Sink`/parser notifications go through `Document` to `Engine::create_frame`
   and back. Initial `about:blank` document at insertion; `srcdoc` parsed with
   html5ever's `iframe_srcdoc` option; per-frame `window.frames`/`length`; the
   iframe load event.
3. **`contentWindow` is a `WindowProxy`.** Return the child realm's window
   object; cross-origin allowlist and `contentDocument` null rule land with
   the first iframe milestone (phase-1 process co-tenancy makes this
   mandatory).
4. **`src` navigation.** The renderer reports a child frame's URL; the browser
   dials and mounts by `FrameId`; same-site frames join the tab's renderer.
5. Later: OOPIF per-site routing, WebDriver `switch to frame`,
   `Symbol.toStringTag` class strings (WPT `exceptions.html` currently fails
   on it), adoption re-prototyping (`node-realm-*` tests), `sandbox`/`allow`.

Decisions made:

- ADR 0014: one `Context` per frame, one `Runtime`/waiter per renderer;
  `WindowProxy` identity; cross-origin allowlist from the first iframe
  milestone; post-connection/removing lifecycle; trees realm-agnostic and
  wrappers shared per node with owner-realm prototypes (Blink's main-world
  model; Gecko keeps one wrapper and outerizes).
- Cargo tests cover product, transport, and engine architecture only;
  web-platform behavior is WPT-only.
- The realm registries live on the engine, never in thread-locals: cached
  JavaScript values must be dropped before the runtime.

Gotchas:

- Build/test only inside `direnv exec .`; targeted WPT is
  `direnv exec . ./tools/wpt/run <files...>`.
- `crates/renderer/src/js/bindings.rs` `realm_tests` is the cross-context
  canary: cross-context `Persistent` restore, shared wrapper identity, and
  registry lifetime all break visibly there.
- `World::document`/`document_mut` borrow the shared store; prefer
  `with_document`/`with_main_document` when a guard would outlive the
  caller's `World` borrow.
- A WPT TIMEOUT in `exceptions.html` is its last test waiting on
  `iframe.onload`; it resolves with step 4/5.
- Engine `Drop` clears frames then the registry while the runtime handle is
  still held; keep that order or QuickJS asserts at `JS_FreeRuntime`.
