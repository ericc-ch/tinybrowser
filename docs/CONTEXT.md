# tinybrowser Domain Terms

## Terms

**NodeId**:
Copyable handle naming exactly one live node in one `Dom`; carries a document id, slot number, and generation. It is the tree identity that crosses crate boundaries. It is not a `PageHandle`.
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
The document-compatibility mode html5ever reports for a parsed page (NoQuirks / LimitedQuirks / Quirks); stored on `Dom` because full quirks makes class/id matching ASCII-case-insensitive.
_Avoid_: compatibility mode, IE mode

**Detach**:
Unlinking a subtree from its parent while keeping every node alive.
_Avoid_: remove (ambiguous with destroy)

**Destroy**:
Recursively freeing a subtree's slots so every handle into it goes stale.
_Avoid_: drop, delete

**TreeSink adapter**:
Thin layer in `browser` (`parse_html`) that translates html5ever tree-construction instructions into `Dom` mutations. `dom` has no html5ever dependency; it still pins `markup5ever` for interned names. Template contents are associated on `Dom`.
_Avoid_: parser glue, binding layer

**QualName**:
Qualified element name: namespace plus optional prefix plus local name. Comes from `markup5ever` (interned; re-exported by dom, pinned to html5ever's version).
_Avoid_: tag name (only the local part)

**Fan-in point**:
The `browser` crate: engine (parser, page, QuickJS). Root `tinybrowser` depends on `browser`, `cdp`, and `webdriver`. Root must not depend on `dom` or `net`. Both protocol crates depend on `browser`, never on each other ([ADR 0007](adrs/0007-engine-charter.md)).
_Avoid_: “only crate that may import two layers” as a religion; `cargo test -p dom` is allowed

**Scope**:
The live node a selector query is rooted at; candidates are its descendants in document order, and the scope itself is never one of its own results, while matching still sees real ancestors above it.
_Avoid_: root (means the document's root element), context

**Pre-insert validity**:
The one gate every dom insertion walks (`append`, `insert_before`), mirroring WHATWG's ensure-pre-insert algorithm by rule (anchors, not step numbers): container-kind parents only, document content model (one element child, doctype first), cycle refusal. Bulk moves (`reparent_children`) into a document answer to the same model over the resulting sequence. Refusals are `DomError::HierarchyRequest` / `CycleForbidden`.
_Avoid_: append checks, validation scattered per method

**Element state**:
A pseudo-class truth (`:disabled`, `:lang(en)`, …) answered by `state.rs` against static markup: fully when markup determines it, otherwise as a documented static subset; states whose context cannot exist in a headless tree (pointer, focus, history) parse but match nothing. `:lang()` document language beyond the `lang` attribute is page/`Dom` state, not `net`.
_Avoid_: pseudo-class handling (that word covers parsing too)

**HTML integration point**:
A foreign-content element where HTML parsing resumes instead of breaking out: SVG `foreignObject`/`desc`/`title` (unconditional), plus MathML `annotation-xml` when its `encoding` says HTML; that last one is answered by our TreeSink from a flag recorded at element creation (`browser`'s `integration_points`), per [html.spec.whatwg.org §13.2.6.6](https://html.spec.whatwg.org/multipage/parsing.html#html-integration-point).
_Avoid_: integration element, breakout point

**Hard seam**:
The `net` crate's public type surface: every name callers see (`Agent`, `RequestBuilder`, `Response`, `Body`, `HeaderMap`, `Method`, `Context`, `NetError`, `WebSocket`) is ours, so a later transport swap cannot leak ureq or tungstenite into `browser`.
_Avoid_: abstraction layer, backend boundary (those mix the type rule with the conversion point)

**HTML job**:
One unit of page work the spec orders: parse, run a script, fire `setTimeout`, deliver a `fetch` callback. Our queue on the page thread, not Tokio's Future list.
_Avoid_: async task (that means a Rust Future)

**Host timer**:
A JS function plus deadline stored by our `setTimeout` implementation. QuickJS does not own it. We call the function later via the engine.
_Avoid_: macrotask inside QuickJS

**Context**:
Which initiator owns a `net` request (`Navigation`, `Fetch`, `Xhr`, `WsHandshake`). SameSite uses it for the Lax top-level navigation exception; schemeful same-site is initiator URL versus request URL. Future `Sec-Fetch-*` headers also key off it.
_Avoid_: scope (dom selector root), initiator (the document URL passed separately)

**Conversion point**:
The few places inside `net` that talk to a backend crate (`AgentBuilder::build`, `RequestBuilder::send`, `RequestBuilder::upgrade`, `Response::from_backend`, `From<ureq::Error>`, `dial::open`, `NetConnector`); nowhere else may mention ureq, native-tls, or tungstenite types.
_Avoid_: adapter, wrapper, FFI boundary

**Host object**:
A page-JS object whose identity and methods are Rust (rquickjs class wrapping a `NodeId` or other page-owned handle). WebIDL interfaces with branding, tree mutation, or a host resource are host objects. ECMAScript-only sugar on top of them may be JS.

Today one `JsNode` host class holds Document, Element, Text, Comment, DocumentType, and DocumentFragment methods. JS constructor functions plus prototype swap provide `instanceof` branding. The wrapper cache is a JS `WeakRef` per interned `NodeId`, so exposing a node does not root it for the document lifetime.

Later: one host object per WebIDL interface ([ADR 0010](adrs/0010-page-actor-ownership.md)).
_Avoid_: polyfill, binding glue, wrapper (those mix host objects with JS-written APIs)

**Page**:
A top-level browsing context (a tab). It keeps its identity across navigation. The actor-owned tab is `Page`; other threads talk to it through `PageHandle` / `PageActor` ([ADR 0010](adrs/0010-page-actor-ownership.md)).
_Avoid_: document (the active document is replaced on navigation)

**Document**:
The active document of a Page. Navigation replaces it. The Page remains.
_Avoid_: page, tab

**PageHandle**:
The value-only handle other threads and protocols use to talk to one `PageActor`. Commands, events, request IDs, values, and explicit errors may cross. DOM references, QuickJS values, callbacks, and closures must not.
_Avoid_: Page (the actor-owned tab), NodeId (tree identity inside the actor)

**PageActor**:
One OS thread per Page, one long-lived current-thread Tokio runtime. Owns DOM, QuickJS realm, wrapper cache, document state, HTML-job queues, timers, and navigation state ([ADR 0010](adrs/0010-page-actor-ownership.md)).
_Avoid_: Page thread as a Spectre boundary (threads isolate ownership and scheduling only)

**Browser**:
One daemon process hosts one Browser bound to one named Profile. Browser owns `ProfileStore`, the shared `NetworkSession`, and the page registry of `PageHandle`s ([ADR 0010](adrs/0010-page-actor-ownership.md)).
_Avoid_: WebDriver session (that is automation state only), Profile as a runtime owner between Browser and pages

**BrowserHandle**:
The value-only handle CDP and WebDriver use to drive Browser. Neither protocol owns Browser, Profile, or NetworkSession ([ADR 0009](adrs/0009-named-profile-daemon.md)).
_Avoid_: Browser (the process-owned engine)

**Profile**:
A named durable browser data set. The implicit name is `default`. One daemon and one Browser per profile ([ADR 0009](adrs/0009-named-profile-daemon.md)).
_Avoid_: session (WebDriver or CDP session), runtime owner of pages

**ProfileStore**:
Durable backing for one Profile under `XDG_DATA_HOME`. Cookies first. Later localStorage, IndexedDB, HTTP cache, and similar site data use the same store. It is not a second live cookie jar. Missing both `XDG_DATA_HOME` and `HOME` is an error; the store does not fall back to `/tmp` ([ADR 0010](adrs/0010-page-actor-ownership.md)).
_Avoid_: cookie jar on `Agent` as the lasting durable owner

**NetworkSession**:
Browser-owned live networking service for one Profile. It wraps one shared `net::Agent`, which keeps the connection pool, transport settings, and live cookie jar. `ProfileStore` is durable backing, not another live jar. The service loads cookies into the jar and writes dirty cookies on navigation, page stop, and session drop. Page actors receive a value-only network/fetch handle; they do not expose or own `net::Agent`. Blocking net calls stay on `spawn_blocking`. Completions return as actor events ([ADR 0010](adrs/0010-page-actor-ownership.md), hard seam [ADR 0006](adrs/0006-net-transport.md)).
_Avoid_: Agent as a second durable owner, page-owned Agent

**Profile daemon**:
The browser process for one named Profile, spawned from the same executable. A CLI command starts it detached when that profile daemon is missing. It binds `127.0.0.1` on a random port. It stops on explicit stop (`Browser.close`), OS user-session exit, or failure. Registration is user-only at `$XDG_RUNTIME_DIR/tinybrowser/<profile>/daemon.json`. A startup lock in that directory elects one daemon if several CLI processes start together ([ADR 0009](adrs/0009-named-profile-daemon.md)).
_Avoid_: separately shipped helper, idle-exit server

**CDP**:
Chrome DevTools Protocol. Control plane for the CLI and external tools. First slice uses Chrome-style local trust: loopback bind, user-only runtime files, no `Host`/`Origin` checks, no websocket token. Honest Browser, Target, Page, and Runtime subsets. Unsupported methods return method-not-found.
_Avoid_: private RPC, pretending to support a method

**Flattened session**:
CDP routing on the browser WebSocket. `Target.attachToTarget` with `flatten` true returns a `sessionId`. Later target commands and events carry `sessionId` at the top JSON level. Direct page sockets need no `sessionId`. Both paths reach the same `PageHandle`.
_Avoid_: a second page object per socket

**WebDriver**:
W3C HTTP automation protocol. This is how wptrunner loads a real document and collects testharness results (`./tools/wpt/run [tests]`, which builds the binary and runs `./wpt run --binary … --ssl-type none tinybrowser`).

A peer adapter over `BrowserHandle` and the selected profile ([ADR 0009](adrs/0009-named-profile-daemon.md)). `--webdriver=PORT` is the in-process WPT host. One active classic HTTP session. The session owns browsing-context selection, timeouts, element references, and input state only. WPT isolation is a temporary profile provided by the runner ([ADR 0008](adrs/0008-wpt-via-webdriver.md)).
_Avoid_: CDP (CLI/agent control, not the WPT driver), WebDriver as owner of Browser or pages

**Resolve map**:
Ordered `--resolve=PATTERN=ADDR` rewrites on `AgentBuilder`; `PATTERN` is an exact host or `*` glob, `ADDR` is an IPv4 literal or `fail`. First match wins. Default is empty (libc DNS). `./tools/wpt/run` passes the `.test` lines and skips WPT’s `/etc/hosts` check; the binary does not remap unless the flag is set.
_Avoid_: hosts file, `/etc/hosts` for WPT

**WPT gate**:
web-platform-tests is the suite for web-visible behavior (DOM, HTML, fetch, cookies, WebSocket as JS sees them). Browser-crate JS/DOM tests stay until testharness actually completes through WebDriver; they are not a second web suite.
_Avoid_: “WPT covers net” (it does not import `net::Agent`; transport unit tests are a different layer)
