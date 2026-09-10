# Host protocol stack: axum + clap, 10 MB size cap

The host's loopback protocol adapters and CLI stop being hand-rolled. Inbound HTTP
for CDP and WebDriver runs on **axum** (hyper/tower); CLI parsing runs on **clap**.
The stripped-binary ceiling moves from 5 MB to **10 MB**.

Status: accepted (2026-09-10). Supersedes the "no axum/hyper" rule and the `http1`
crate in [ADR 0007](0007-engine-charter.md). The engine charter otherwise stands:
the page runtime is still Tokio current-thread `rt`+`time`, blocking page work runs
on the browser-owned executor, and the renderer path takes no web-server stack.

## Decision

- CDP and WebDriver inbound HTTP is served by **axum** on a Tokio runtime. CDP
  WebSockets use axum's `ws` support. `net`'s outbound WebSocket client stays on
  tungstenite ([ADR 0006](0006-net-transport.md)).
- CLI parsing is **clap** (derive). `--webdriver=PORT`, `--profile=NAME`,
  `--resolve=PATTERN=ADDR`, and `--daemon` keep working; `create`, `list`, `select`,
  `eval`, `navigate`, and `close` become real subcommands.
- The `http1` crate is deleted once both adapters are ported.
- The size budget is **10 MB stripped x86_64** (was 5 MB), tracked in AGENTS.md and
  `docs/researches/size-budget.md`. Milestones still measure marginals; the cap is a
  ceiling, not a target, and regressions still justify themselves in bytes.
- No HTTP framework, CLI crate, or multi-thread runtime enters the renderer. The renderer's `protocol` module uses `serde`/`serde_json` for the value-only IPC seam ([ADR 0011](0011-renderer-processes-per-site.md)); nothing else in the renderer serializes. Renderer IPC is std IPC, not HTTP.
- The renderer runtime keeps Tokio current-thread `rt`+`time`. The host may take
  `rt-multi-thread` + `net` for the server; host page actors are plain OS threads
  and renderers keep their own current-thread runtimes.

## Why

- The old `http1` crate is request-line, headers, and `Content-Length` only. CDP and
  WebDriver need chunked bodies, keep-alive edge cases, upgrade handling, and
  WebSocket framing. Reimplementing those is where protocol bugs live, and the
  loopback trust model does not change that.
- Hand-rolled CLI parsing produced bespoke error strings and a `USAGE` constant as a
  catch-all error. clap gives one parse path, real help, and shell completion later.
- The 5 MB cap forced these decisions more than the product did. At the single
  measured engine baseline the binary is 3.5 MB; a 10 MB cap keeps the engine lean
  while letting the host layer use maintained crates.

## Consequences

- Binary grows. Expected axum + hyper + tower + Tokio multi-thread + clap: roughly
  +1.5–2.5 MB tuned. Measure at the port and record it in
  `docs/researches/size-budget.md`.
- `http1` leaves the workspace, its tests, and `CONTEXT.md`.
- Blocking `BrowserHandle`/`PageHandle` calls inside async handlers go through
  `tokio::task::spawn_blocking`, never a blocking `send()` on a runtime worker.
- The daemon and `--webdriver` mode each build a Tokio runtime for the server. Page
  actors keep their own current-thread runtimes; no runtime is nested inside
  another.
- `cargo test` CLI flag-error cases change with clap's messages; the tests assert
  clap's behavior instead of the hand-rolled strings.

## Options considered

- **Keep `http1` and hand-rolled parsing:** smallest binary, but keeps protocol
  correctness on us and the cap was the only real argument. Rejected.
- **hyper directly, no axum:** lighter, but more glue for routing, extractors, and
  the WebSocket upgrade; axum is the maintained path on top of it. Rejected.
- **tiny_http / blocking server:** fits the existing sync style, but no first-class
  WebSocket story for CDP and another semi-maintained dependency. Rejected.
- **clap builder API instead of derive:** no proc-macro growth, but more code and no
  compile-time structure. Rejected; derive is the maintained default.
- **Separate `cli` crate:** the root crate is the embedder; one binary. A crate
  boundary would not buy a dependency rule. Rejected.
