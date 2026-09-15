# Engine charter

Crate slogans, a fake `js` seam, and parking DOM/language on `net` were fighting the product: a small page engine with a task loop, not a web-server org chart. The decided shape is three deep crates, tasks on a Tokio current-thread waiter, and `renderer` holding `parse_html`. Browser owns `NetworkSession` ([ADR 0010](0010-page-actor-ownership.md)).

Status: accepted and amended by
[ADR 0019](0019-async-browser-runtime-and-io.md). Does not reopen
[ADR 0002](0002-dom-layer-architecture.md). [ADR 0009](0009-named-profile-daemon.md)
takes named profiles and CDP as the CLI. [ADR 0010](0010-page-actor-ownership.md)
takes autonomous page scheduling, parsing, Browser ownership, and persistence.
[ADR 0011](0011-renderer-processes-per-site.md) moves the page engine into the
`renderer` crate and processes. [ADR 0012](0012-host-protocol-and-cli-stack.md)
retires `http1` for axum and clap. ADR 0019 replaces the blocking browser shell,
network executor, and restricted browser-side Tokio use. Public `net` types and
size and lint bounds stay.

| crate | depends on | charter |
|---|---|---|
| `dom` | n/a | arena, `NodeId`, selectors |
| `net` | n/a | async HTTP/WebSocket + live cookie jar; public types ours ([ADR 0019](0019-async-browser-runtime-and-io.md)) |
| `renderer` | `dom` | page engine: TreeSink, `Document` + QuickJS, value-only browser seam; never `net` ([ADR 0011](0011-renderer-processes-per-site.md)) |
| `browser` | `net`, `renderer` | browser side: Browser, tab `Tab`, tab registry, `NetworkSession`, renderer process manager ([ADR 0010](0010-page-actor-ownership.md), [ADR 0011](0011-renderer-processes-per-site.md)) |
| `cdp` | `browser`, `axum` | CDP server/client adapter; no direct `dom`, `net`, `renderer`, or `webdriver` dependency ([ADR 0009](0009-named-profile-daemon.md), [ADR 0012](0012-host-protocol-and-cli-stack.md)) |
| `webdriver` | `browser`, `axum` | classic WebDriver adapter over `BrowserHandle`; WPT endpoint ([ADR 0008](0008-wpt-via-webdriver.md)) |
| root `tinybrowser` | `browser`, `cdp`, `webdriver`, `renderer` | embedder + bins; `renderer` only for the `renderer` subcommand |

One compile-error law: `cdp` and `webdriver` must not depend on each other, `dom`, `net`, or `renderer`. `cargo test -p` a leaf crate is not reach-around. QuickJS lives in `renderer`. No `js` crate. No `HttpTransport` trait.

The renderer uses one Tokio current-thread runtime as an IPC and timer waiter.
Tasks and microtasks stay in the page engine's queues. The renderer may enable
Tokio's current-thread runtime, timer, synchronization, macro, networking, and
I/O utility features. It does not use Tokio `full`, a multi-thread runtime,
hyper, axum, or another async runtime. The browser process owns one multi-thread
Tokio runtime and uses hyper-util through `net`. See
[ADR 0019](0019-async-browser-runtime-and-io.md).

Template contents live on `Dom`. The live cookie jar stays on `Agent`. `NetworkSession` loads and persists that jar through `ProfileStore`. `document.cookie` and `Content-Language` are document-level. CDP does not own cookies. Durable named profiles and the CDP control plane are in [ADR 0009](0009-named-profile-daemon.md).

## Options considered

- **Monolith with drawn seams:** illegal imports are invisible; only viable for a throwaway or an experienced reviewer. Rejected. Crates exist so a `[dependencies]` line is a greppable build error.
- **Seven-crate shape with `cli`/`lib` shells and a `core` layer:** `core` was bypassed, so `cdp` imported `dom`/`js`/`net` directly. Rejected.
- **Four crates including empty `js`, root depending on all four, plus an `HttpTransport` trait:** over-policing and compile theater. Replaced by the table above.
- **No Tokio, hand-rolled park/wake:** smaller, easy to get timers wrong. Rejected; `rt`+`time` is ~+66 KB.
- **Tokio `full`:** retains features neither process needs. Rejected. Axum runs only in browser-process protocol adapters. Hyper-util also runs browser-side as net v2's maintained outbound HTTP stack ([ADR 0019](0019-async-browser-runtime-and-io.md)).
- **Stealth in the next coding push:** fails the size/sequence goal. Deferred, not dropped.
