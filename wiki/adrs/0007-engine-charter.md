# Engine charter

Crate slogans, a fake `js` seam, and parking DOM/language on `net` were fighting the product: a small page engine with a job loop, not a web-server org chart. The decided shape is three deep crates, HTML jobs on a Tokio current-thread waiter, `browser` holding `net::Agent` and `parse_html`.

Status: accepted. Source: [engine-charter spec](../works/engine-charter/spec.md). Supersedes the crate table and “js must not depend on net” / root-depends-on-all-four rules in [ADR 0001](0001-workspace-crates-with-enforced-edges.md). Does not reopen [ADR 0002](0002-dom-layer-architecture.md) arena or [ADR 0006](0006-net-transport.md) v1 transport.

| crate | depends on | charter |
|---|---|---|
| `dom` | n/a | arena, `NodeId`, selectors |
| `net` | n/a | blocking HTTP/WS + cookie jar; public types ours ([ADR 0006](0006-net-transport.md)) |
| `browser` | `dom`, `net` | engine: TreeSink, later page + QuickJS; holds `Agent` |
| root `tinybrowser` | `browser` | embedder + bins |

One compile-error law: future `cdp` depends on `browser` alone. `cargo test -p` a leaf crate is not reach-around. No `js` crate until QuickJS has a small public surface (the empty workspace member is leftover). No `HttpTransport` trait.

Page thread: Tokio current-thread, features `rt` + `time` only. HTML tasks and microtasks are our queue. `send` / `upgrade` only via `spawn_blocking`. No tokio `full`, smol, axum, hyper. Stealth (Chrome TLS/h2) is later later.

Template contents live on `Dom`. Cookie jar stays on `Agent`; `document.cookie` and `Content-Language` are page/document. Persistence is in-memory until a profile, not CDP.

## Options considered

- **Empty `js` crate + `HttpTransport`:** compile theater; one implementor. Rejected.
- **No Tokio, hand-rolled park/wake:** smaller, easy to get timers wrong. Rejected; `rt`+`time` is ~+66 KB.
- **Tokio `full` / axum / hyper:** server stack. Rejected for size and for the wrong product.
- **Stealth in the next coding push:** fails the size/sequence goal. Deferred, not dropped.
