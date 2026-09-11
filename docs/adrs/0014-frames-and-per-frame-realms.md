# Frames and per-frame realms

The renderer process becomes a **frame host**: one QuickJS `Runtime` (heap) and
one Tokio waiter are shared by every frame in the process, while each frame owns
its own `Document` and its own QuickJS `Context` (realm). The `iframe` element
creates and destroys frames through DOM lifecycle steps, and the browser process
owns the authoritative frame tree that routes per-frame navigation.

Status: accepted. Extends [ADR 0011](0011-renderer-processes-per-site.md);
the site-instance cut it defined was chosen for exactly this feature
("finer than a tab, coarser than an origin"). Replaces the one-document-per-
process shape of the `renderer` crate. Does not change `TabHandle`, `TabId`, or
the protocol adapters.

## Decision

- **One QuickJS `Runtime` per renderer process; one `Context` per frame.** QuickJS
  contexts of one runtime share objects, "similar to frames of the same origin"
  ([QuickJS docs](https://bellard.org/quickjs/quickjs.html), JSRuntime section).
  This matches the HTML spec: every navigable gets a new realm whose global is a
  new `Window` object
  ([create a new browsing context and document](https://html.spec.whatwg.org/multipage/document-sequences.html#creating-a-new-browsing-context)),
  and the agent cluster key is the **site**
  ([agent cluster key](https://html.spec.whatwg.org/multipage/webappapis.html#agent-cluster-key)),
  so same-site frames belong to one agent while cross-site frames do not.
- **The page engine (`renderer::Engine`) owns process-wide state:** the QuickJS
  `Runtime`, one Tokio current-thread runtime as the waiter, the frame registry,
  and the command/pump loops. `Document` stops owning either runtime; it is one
  frame's tree, realm, parser, tasks, and timers.
- **`FrameId` addresses a frame inside one renderer process.** It is minted by the
  engine and carried by commands and events that target a frame. The browser
  process keeps the authoritative frame tree (parent, container element, site,
  URL, load state) and routes navigation to the renderer for the frame's site.
- **`contentWindow` returns a `WindowProxy`, not the frame's global object.** The
  proxy identity survives navigation, per
  [the WindowProxy exotic object](https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-windowproxy-exotic-object).
  `contentDocument` returns null unless the container document and the frame
  document are same origin-domain
  ([content document](https://html.spec.whatwg.org/multipage/document-sequences.html#concept-bcc-content-document)).
  Cross-origin `WindowProxy` access is limited to the spec allowlist
  (`postMessage`, `location` setter, `close`, `frames`, `length`, ...)
  ([cross-origin accessible window property name](https://html.spec.whatwg.org/multipage/nav-history-apis.html#cross-origin-accessible-window-property-name)).
  These checks land with the first iframe milestone, independent of process
  isolation.
- **DOM lifecycle steps drive frame lifetime.** `dom` exposes the spec hooks:
  *insertion steps* (no tree mutation), *post-connection steps* (may mutate the
  tree, run after the insertion traversal), and *removing steps*
  ([DOM](https://dom.spec.whatwg.org/#concept-node-insert),
  [post-connection](https://dom.spec.whatwg.org/#concept-node-post-connection-ext),
  [removing](https://dom.spec.whatwg.org/#concept-node-remove-ext)). An `iframe`
  creates its content navigable in post-connection steps and destroys it in
  removing steps, exactly as the spec's
  [iframe hooks](https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element)
  require. Blink splits the same two phases (`InsertedInto` queues,
  `DidNotifySubtreeInsertionsToDocument` opens the URL); Gecko uses
  `BindToTree`/`UnbindFromTree`.
- **Process isolation for cross-site frames is phased, access checks are not.**
  The first implementation hosts all frames of a tab in the tab's renderer.
  OOPIF (a renderer per cross-site frame site) is the next milestone. The
  cross-origin property checks and the `contentDocument` null rule are not
  deferred: without them, co-tenancy leaks immediately.

## Consequences

- Limits become process-wide, not frame-wide: the 32 MiB QuickJS heap and the
  interrupt handler are per `Runtime`, so one renderer's frames share the memory
  budget and the execution deadline machinery.
- `RemoteValue::Node` interning moves from per-`Document` to per-renderer, since
  WebDriver element ids come from it and two frames must not emit the same id.
- `RendererRegistry` pooling changes: a renderer hosting live frames cannot be
  returned to the idle pool by site; frame lifetimes, not tab navigation alone,
  decide when a renderer is releasable.
- The single-document public API of the `renderer` crate changes. The in-process
  backend, the `--renderer` child, and the renderer integration tests all drive
  an `Engine`.
- The renderer keeps one Tokio current-thread runtime and pumps all frames from
  it; `Document::run`'s per-document Tokio runtime and its "not inside another
  runtime" assertion are replaced by engine-level pumping.

## Options considered

- **One context per tab with window-proxy objects (no second realm).** Rejected:
  per-realm constructors and `DOMException` identity are observable and tested
  (`iframe.contentWindow.DOMException`, `new iframe.contentWindow[ctor]()`),
  and top-level `var` would leak between frames.
- **One renderer process per frame from the start.** Rejected for the first
  milestone: cross-site frames need no synchronous access, but per-site routing,
  registry rework, and cross-process WindowProxy identity are a larger seam
  change; phasing keeps the first milestone verifiable while designing the seam
  frame-addressed.
- **Wait for a QuickJS multi-realm-per-context extension.** Not needed; QuickJS
  contexts sharing a runtime is the intended same-origin-frames model, and
  rquickjs already exercises cross-context persistents and function calls in its
  own test suite.
- **Parser-only iframe handling (no DOM lifecycle hooks).** Rejected: dynamic
  `appendChild(iframe)` must create a content navigable synchronously, and the
  WPT `insertion-removing-steps` subtests assert the exact timing; custom
  elements will reuse the same hooks later.
