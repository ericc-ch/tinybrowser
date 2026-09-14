# Browser-process protocol stack: axum + clap, 10 MB size cap

The browser process's loopback protocol adapters and CLI stop being hand-rolled. Inbound HTTP
for CDP and WebDriver runs on **axum** (hyper/tower); CLI parsing runs on **clap**.
The stripped-binary ceiling moves from 5 MB to **10 MB**.

Status: accepted (2026-09-10) and amended by
[ADR 0019](0019-async-browser-runtime-and-io.md). Supersedes the "no axum/hyper"
rule and the `http1` crate in [ADR 0007](0007-engine-charter.md). ADR 0019 moves
runtime ownership to the executable, makes the adapters async, and uses
hyper-util for outbound browser networking. The renderer still takes no
web-server stack or multi-thread runtime. Axum stays during the core migration.
The final server-stack checkpoint measures axum against direct hyper and records
the resulting choice.

## Decision

- CDP and WebDriver inbound HTTP is served by **axum** on the browser process's Tokio runtime. CDP
  WebSockets use axum's `ws` support. `net`'s outbound WebSocket client uses
  Tokio and `tokio-tungstenite` ([ADR 0019](0019-async-browser-runtime-and-io.md)).
- CLI parsing is **clap** (derive). Process modes are subcommands: `daemon`, `renderer`, and `webdriver --port=PORT`. `--profile=NAME` lives on `daemon` and `webdriver`; `--resolve=PATTERN=ADDR` lives on `webdriver`. `--log-level`, `--verbose`, and `--version` stay global. `create`, `list`, `select`, `eval`, `navigate`, and `close` become real subcommands later.
- The `http1` crate is deleted once both adapters are ported.
- The size budget is **10 MB stripped x86_64** (was 5 MB), tracked in AGENTS.md and
  `docs/researches/size-budget.md`. Milestones still measure marginals; the cap is a
  ceiling, not a target, and regressions still justify themselves in bytes.
- No HTTP framework, CLI crate, or multi-thread runtime enters the renderer. The renderer's `protocol` module uses `serde`/`serde_json` for small control payloads on the value-only IPC seam ([ADR 0019](0019-async-browser-runtime-and-io.md)); raw body chunks do not use JSON.
- The renderer keeps one Tokio current-thread runtime. The renderer enables only
  the features needed for its waiter and private platform channel. The browser
  process owns one multi-thread runtime for adapters, browser tasks, tabs,
  networking, and renderer channels.

## Why

- The old `http1` crate is request-line, headers, and `Content-Length` only. CDP and
  WebDriver need chunked bodies, keep-alive edge cases, upgrade handling, and
  WebSocket framing. Reimplementing those is where protocol bugs live, and the
  loopback trust model does not change that.
- Hand-rolled CLI parsing produced bespoke error strings and a `USAGE` constant as a
  catch-all error. clap gives one parse path, real help, and shell completion later.
- The 5 MB cap forced these decisions more than the product did. At the single
  measured engine baseline the binary is 3.5 MB; a 10 MB cap keeps the engine lean
  while letting the browser-process layer use maintained crates.

## Consequences

- Binary grows. Expected axum + hyper + tower + Tokio multi-thread + clap: roughly
  +1.5–2.5 MB tuned. Measure at the port and record it in
  `docs/researches/size-budget.md`.
- `http1` leaves the workspace, its tests, and `CONTEXT.md`.
- `BrowserHandle` and `TabHandle` are async-only. Axum handlers await them
  directly.
- The executable builds one Tokio runtime for the browser process. CDP and
  WebDriver receive the runtime context and never nest a private runtime.
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
