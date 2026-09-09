# Browser, Profile, and PageActor

One profile daemon hosts one Browser bound to one named Profile. Browser owns `ProfileStore`, the shared `NetworkSession`, and the page registry of `PageHandle`s. A `PageHandle` addresses a `PageActor`. Profile is the named durable identity, not a runtime owner between Browser and pages.

Status: accepted. Keeps [ADR 0007](0007-engine-charter.md) Tokio `rt`+`time`, HTML jobs on our queue, `spawn_blocking` for `send` / `upgrade`, and crate/lint/size bounds. Keeps the [ADR 0006](0006-net-transport.md) hard seam. Durable profile files, daemon lifetime, CDP, and WebDriver are in [ADR 0009](0009-named-profile-daemon.md).

## Ownership

`Browser` owns `ProfileStore`, the shared `NetworkSession`, and the page registry.

A `Page` is a top-level browsing context, a tab identity. It survives navigation. Its active `Document` is replaced on navigation.

Each page is a `PageActor` on one OS thread. That thread has one long-lived current-thread Tokio runtime. The actor owns DOM, the QuickJS realm, the wrapper cache, document state, HTML-job queues, timers, and navigation state.

`PageHandle` communicates with `PageActor` only through commands, events, request IDs, values, and explicit errors. No DOM references, QuickJS values, callbacks, or closures cross the boundary.

`BrowserHandle` is the value-only handle protocols use to drive Browser.

## NetworkSession

`NetworkSession` is the browser-owned live networking service. It wraps one shared `net::Agent`, which keeps the connection pool, transport settings, and live cookie jar. Public `net` types stay ours ([ADR 0006](0006-net-transport.md)).

`ProfileStore` is durable backing, not another live jar. `NetworkSession` loads persisted cookies into `Agent` and saves jar changes through `ProfileStore`.

Page actors receive a value-only network/fetch handle. They do not expose or own `net::Agent`. Blocking net calls stay off the page thread through `spawn_blocking`. Completions return as actor events.

## Persistence

`ProfileStore` owns every durable web-data feature the engine supports. Cookies first. Later localStorage, IndexedDB, HTTP cache, and similar site data use the same store. CDP does not own cookies.

The live jar is marked dirty on cookie changes. `ProfileStore` writes the cookie file on navigation, page stop, and `NetworkSession` drop (including `Browser.close`). `document.cookie` does not write disk on the page thread.

Open tabs, active documents, JavaScript heaps, `sessionStorage`, and in-flight requests are not restored after the daemon restarts.

## Isolation

Threads improve scheduling and ownership isolation. They are not a Spectre security boundary.

Keep one process with page threads for now. A later security phase may self-spawn the same executable into sandboxed renderer processes, preferably isolated by site rather than blindly one process per tab. The value-only `PageHandle` boundary is the seam that later IPC would reuse.

## Host objects

Wrapper caches and host objects live on the actor. One `JsNode` host class plus JS constructor branding is the first slice. Later: one host object per WebIDL interface. The wrapper cache is a JS `WeakRef`, so exposing a node to JavaScript does not root that node for the document lifetime.

## Options considered

- **Keep `Page` on the embedder thread with a fresh runtime per `run`:** Runtime drop waits for started `spawn_blocking` work, so `run_until_load` can wait on unrelated slow fetches. Rejected as the lasting shape.
- **Closures or `NodeId` borrows across the actor boundary:** convenient now, expensive to undo when renderers become processes. Rejected.
- **One OS process per tab now:** cost and complexity before a security design. Rejected. Site-isolated renderer processes are a later phase.
- **Treat page threads as a Spectre boundary:** they isolate scheduling and ownership only.
- **Two authoritative cookie jars:** live `Agent` jar plus a separate store jar. Rejected. One live jar, durable backing in `ProfileStore`.
- **Strong wrapper cache for the document lifetime:** a strong `Persistent` map. Rejected as the lasting shape. Use a weak cache.

## Consequences

- WebDriver and CDP both drive `PageHandle` through `BrowserHandle`. Neither holds DOM or QuickJS values.
- `Page::block_on_pump` and `Page::execute_script` are internal to the actor.
- `Page` stays one owner. The `page` module is split by job (`pump`, `navigate`, `intern`) without extra types.
