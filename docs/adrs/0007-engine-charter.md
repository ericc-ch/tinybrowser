# Engine charter

Crate slogans, a fake `js` seam, and parking DOM/language on `net` were fighting the product: a small page engine with a job loop, not a web-server org chart. The decided shape is three deep crates, a zero-dep inbound HTTP/1.1 helper for protocol adapters, HTML jobs on a Tokio current-thread waiter, and `browser` holding `parse_html`. Browser owns `NetworkSession` ([ADR 0010](0010-page-actor-ownership.md)).

Status: accepted. Does not reopen [ADR 0002](0002-dom-layer-architecture.md) arena or [ADR 0006](0006-net-transport.md) v1 transport. [ADR 0009](0009-named-profile-daemon.md) takes named profiles and CDP as the CLI. [ADR 0010](0010-page-actor-ownership.md) takes autonomous page scheduling, incremental parsing, Browser ownership, and the bounded network executor. [ADR 0011](0011-renderer-processes-per-site.md) moves the page engine into the `renderer` crate and processes; [ADR 0012](0012-host-protocol-and-cli-stack.md) retires `http1` for axum/clap. Crate graph below is amended accordingly; Tokio `rt`+`time`, public `net` types, and size/lint bounds stay.

| crate | depends on | charter |
|---|---|---|
| `dom` | n/a | arena, `NodeId`, selectors |
| `net` | n/a | blocking HTTP/WS + live cookie jar; public types ours ([ADR 0006](0006-net-transport.md)) |
| `renderer` | `dom` | page engine: TreeSink, `Document` + QuickJS, value-only host seam; never `net` ([ADR 0011](0011-renderer-processes-per-site.md)) |
| `browser` | `net`, `renderer` | host: Browser, tab `Page`, page registry, `NetworkSession`, renderer registry/backends ([ADR 0010](0010-page-actor-ownership.md), [ADR 0011](0011-renderer-processes-per-site.md)) |
| `cdp` | `browser`, `axum` | CDP server/client adapter; no direct `dom`, `net`, `renderer`, or `webdriver` dependency ([ADR 0009](0009-named-profile-daemon.md), [ADR 0012](0012-host-protocol-and-cli-stack.md)) |
| `webdriver` | `browser`, `axum` | classic WebDriver adapter over `BrowserHandle`; WPT host ([ADR 0008](0008-wpt-via-webdriver.md)) |
| root `tinybrowser` | `browser`, `cdp`, `webdriver`, `renderer` | embedder + bins; `renderer` only for the hidden `--renderer` mode |

One compile-error law: `cdp` and `webdriver` must not depend on each other, `dom`, `net`, or `renderer`. `cargo test -p` a leaf crate is not reach-around. QuickJS lives in `renderer`. No `js` crate. No `HttpTransport` trait.

Renderer thread: Tokio current-thread, features `rt` + `time` only. HTML jobs and microtasks are our queue. Blocking `send` work runs on the bounded Browser-owned executor, reached through the host seam, never the renderer thread. No tokio `full` or smol. axum/hyper live only in the host protocol adapters ([ADR 0012](0012-host-protocol-and-cli-stack.md)). Stealth (Chrome TLS/h2) is later later. Each renderer has a long-lived current-thread runtime ([ADR 0010](0010-page-actor-ownership.md), [ADR 0011](0011-renderer-processes-per-site.md)).

Template contents live on `Dom`. The live cookie jar stays on `Agent`. `NetworkSession` loads and persists that jar through `ProfileStore`. `document.cookie` and `Content-Language` are page/document. CDP does not own cookies. Durable named profiles and the CDP control plane are in [ADR 0009](0009-named-profile-daemon.md).

## Options considered

- **Monolith with drawn seams:** illegal imports are invisible; only viable for a throwaway or an experienced reviewer. Rejected. Crates exist so a `[dependencies]` line is a greppable build error.
- **Seven-crate shape with `cli`/`lib` shells and a `core` layer:** `core` was bypassed, so `cdp` imported `dom`/`js`/`net` directly. Rejected.
- **Four crates including empty `js`, root depending on all four, plus an `HttpTransport` trait:** over-policing and compile theater. Replaced by the table above.
- **No Tokio, hand-rolled park/wake:** smaller, easy to get timers wrong. Rejected; `rt`+`time` is ~+66 KB.
- **Tokio `full` / axum / hyper:** server stack. Rejected here for size and for the wrong product; axum/hyper later entered the host protocol adapters only ([ADR 0012](0012-host-protocol-and-cli-stack.md)).
- **Stealth in the next coding push:** fails the size/sequence goal. Deferred, not dropped.
