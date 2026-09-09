# Engine charter

Crate slogans, a fake `js` seam, and parking DOM/language on `net` were fighting the product: a small page engine with a job loop, not a web-server org chart. The decided shape is three deep crates, a zero-dep inbound HTTP/1.1 helper for protocol adapters, HTML jobs on a Tokio current-thread waiter, and `browser` holding `parse_html`. Browser owns `NetworkSession` ([ADR 0010](0010-page-actor-ownership.md)).

Status: accepted. Supersedes the crate table and “js must not depend on net” / root-depends-on-all-four rules in [ADR 0001](0001-workspace-crates-with-enforced-edges.md). Does not reopen [ADR 0002](0002-dom-layer-architecture.md) arena or [ADR 0006](0006-net-transport.md) v1 transport. [ADR 0009](0009-named-profile-daemon.md) takes named profiles and CDP as the CLI. [ADR 0010](0010-page-actor-ownership.md) takes Browser ownership, `NetworkSession`, and the long-lived page runtime. Crate graph, Tokio `rt`+`time`, `spawn_blocking`, public `net` types, and size/lint bounds stay.

| crate | depends on | charter |
|---|---|---|
| `dom` | n/a | arena, `NodeId`, selectors |
| `net` | n/a | blocking HTTP/WS + live cookie jar; public types ours ([ADR 0006](0006-net-transport.md)) |
| `http1` | n/a | inbound HTTP/1.1 on a stream for loopback protocol adapters; no hyper/axum |
| `browser` | `dom`, `net` | engine: TreeSink, page + QuickJS. Browser owns `NetworkSession` ([ADR 0010](0010-page-actor-ownership.md)) |
| `cdp` | `browser`, `http1` | CDP server/client adapter; no direct `dom`, `net`, or `webdriver` dependency ([ADR 0009](0009-named-profile-daemon.md)) |
| `webdriver` | `browser`, `http1` | classic WebDriver adapter over `BrowserHandle`; WPT host ([ADR 0008](0008-wpt-via-webdriver.md)) |
| root `tinybrowser` | `browser`, `cdp`, `webdriver` | embedder + bins |

One compile-error law: `cdp` and `webdriver` may depend on `http1`. They must not depend on each other, `dom`, or `net`. `cargo test -p` a leaf crate is not reach-around. QuickJS lives in `browser`. No `js` crate. No `HttpTransport` trait.

Page thread: Tokio current-thread, features `rt` + `time` only. HTML jobs and microtasks are our queue. `send` / `upgrade` only via `spawn_blocking`. No tokio `full`, smol, axum, hyper. Stealth (Chrome TLS/h2) is later later. Each page actor has a long-lived current-thread runtime ([ADR 0010](0010-page-actor-ownership.md)).

Template contents live on `Dom`. The live cookie jar stays on `Agent`. `NetworkSession` loads and persists that jar through `ProfileStore`. `document.cookie` and `Content-Language` are page/document. CDP does not own cookies. Durable named profiles and the CDP control plane are in [ADR 0009](0009-named-profile-daemon.md).

## Options considered

- **Empty `js` crate + `HttpTransport`:** compile theater; one implementor. Rejected.
- **No Tokio, hand-rolled park/wake:** smaller, easy to get timers wrong. Rejected; `rt`+`time` is ~+66 KB.
- **Tokio `full` / axum / hyper:** server stack. Rejected for size and for the wrong product.
- **Stealth in the next coding push:** fails the size/sequence goal. Deferred, not dropped.
