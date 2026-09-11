# Handoff (2026-09-11)

State: Branch `wpt-pass-chasing`, HEAD `3de769a`. ADR 0014 steps 1-3 are
committed and green (`cargo test --workspace` 27 suites, clippy, fmt). Frame
hosting exists at every layer; iframes themselves are not created yet.

Done (this session, each commit verified):

- `488371c` `renderer::Engine` hosts one QuickJS `Runtime` and one Tokio
  waiter for the whole renderer process; `Document` is one frame sharing both.
  `run_with_stop` drives the engine. `DOMException` inherits
  `Error.prototype` per WebIDL, so `String(exception)` is `"Name: message"`.
  Browser-conformance cargo stand-ins were deleted (Node-name, collections,
  MutationObserver, DOMImplementation, DOMException branding); WPT is the
  gate (see the WPT gate term in `docs/CONTEXT.md`).
- `9083c1e` the engine mints `FrameId`s, keeps a frame registry, and pumps
  every frame from the shared waiter. The JS world is registered per QuickJS
  context; runtime userdata is one slot per process and used to clobber the
  first frame's world. Unit test `realm_tests` proves two realms on one heap
  resolve their own world and call a function created in the other realm.
- `3de769a` `Command::Mount`/`Eval`/`ExecuteScript`/`SetDocumentUrl` and
  `FromRenderer::Event` carry `FrameId`; renderer-link subscribers receive
  `(FrameId, TabEvent)`; `Tab` tracks load per frame;
  `TabError::UnknownFrame` reports stale frames. The browser still commands
  only `FrameId::MAIN`.

In flight: none.

Next (ADR 0014 steps 4-5):

1. **Cross-realm document resolution — the critical path.** When a wrapper
   created in realm A is read from realm B, the getter runs with B's `Ctx`,
   and all ~91 `world(&ctx)` sites in `bindings.rs` resolve the caller's
   World; a foreign `NodeId` is therefore looked up in the wrong World.
   Document ids are globally unique (`dom/src/arena.rs` `NEXT_DOCUMENT_ID`),
   so nothing aliases, but every cross-realm getter throws. Design: register
   documents renderer-wide (`document id -> Weak<Rc<RefCell<World>>>`) and
   add `world_for(ctx, id)`, then resolve the receiver's world from
   `self.handle` in node/document methods. `contentDocument`/`contentWindow`
   must return the child realm's own objects; the realm test proves
   cross-context `Persistent` restore and calls already work. Decide wrapper
   identity (renderer-wide vs per realm) here: same-origin frames in Blink
   share the main world, which argues for renderer-wide wrappers.
2. **DOM lifecycle hooks.** `dom` has no insertion/post-connection/removing
   steps. Add them (iframe creates its content navigable in post-connection,
   destroys it in removing). WPT `insertion-removing-steps` probes this
   timing; Blink splits `InsertedInto` / `DidNotifySubtreeInsertionsToDocument`
   the same way.
3. **iframe bindings + srcdoc.** `HTMLIFrameElement` members (`contentWindow`,
   `contentDocument`, `srcdoc`, `name`), frame creation at post-connection,
   an initial about:blank document at insertion, `srcdoc` parsed with
   html5ever's `iframe_srcdoc` option, per-frame `window.frames`/`length`,
   the iframe load event.
4. **`src` navigation.** The renderer reports a child frame's URL; the browser
   dials and mounts by `FrameId`; same-site frames join the tab's renderer.
5. Later: OOPIF per-site routing, WebDriver `switch to frame`,
   `Symbol.toStringTag` class strings (WPT
   `webidl/ecmascript-binding/es-exceptions/exceptions.html` currently fails
   on it), `sandbox`/`allow`, lazy loading.

Decisions made:

- ADR 0014: one QuickJS `Context` per frame, one `Runtime`/waiter per
  renderer; `WindowProxy` identity survives navigation; cross-origin
  allowlist and `contentDocument` null rule land with the first iframe
  milestone even while process isolation is phased; post-connection and
  removing steps drive frame lifetime.
- Cargo tests cover product, transport, and engine architecture only;
  web-platform behavior is tested by WPT.
- Runtime userdata is not per realm; worlds live in a context-pointer
  registry (thread-local, one renderer thread).

Gotchas:

- Build/test only inside `direnv exec .`; `cargo test --workspace` is the fast
  gate, targeted WPT is `direnv exec . ./tools/wpt/run <files...>`.
- `crates/renderer/src/js/bindings.rs` `realm_tests` is the cross-context
  canary: if cross-context `Persistent` restore or class prototypes regress,
  iframes lose cross-frame access.
- A WPT TIMEOUT in `exceptions.html` is its last test waiting on
  `iframe.onload`; it resolves with steps 3-4.
- `docs/researches/engine-source.md` has the fetch rules for Firefox/Blink
  ground truth; `docs/CONTEXT.md` defines Frame, FrameId, WindowProxy, Realm.
