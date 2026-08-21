We are building the smallest headless browser for AI agents. Engine stops at DOM + JS. No layout or screenshots.

## Vision

- Single binary, no cheating: no sidecar processes. Sub-5MB stripped x86_64 binary is the goal. Measure size at milestones.
- Stealth by default, no opt-out.
- CDP is the agent boundary: full Chromium surface; no-op stub domains stay for Puppeteer/Playwright compatibility. No bespoke SDK, no C ABI.
- Embeddable = spawn-and-drive binary, or the `tinybrowser-lib` crate as thin in-process wrapper. Keep the wrapper as-is; it costs zero binary size.
- CLI: `serve` and `fetch` are load-bearing (e2e gate drives `fetch`).

## Priority & Layering

Build bottom-up; bring up CLI `fetch` with Core to gate changes via `e2e/run.sh`:

1. Foundation (`dom`, `net`): DOM arena (`slotmap`), selectors, parser, HTTP/TLS stealth, cookies.
2. Runtime (`js`): QuickJS, native DOM bindings (`rquickjs::class`), WebCrypto, private shim.
3. Core & CLI (`core`, `cli`): `PageActor`, navigation lifecycles, and the CLI (used for e2e runner, first class abstraction).
4. Protocols (`cdp`, `lib`): CDP WebSocket dispatch and in-process wrapper.

## Verify

A change is done when:

1. `cargo nextest run -p <crate>` is green for every crate you changed.
2. `cargo nextest run --no-fail-fast --workspace` is green.
3. `e2e/run.sh` exits 0 (33 required stages; `observer-intersection` may fail).

## Further Reading

- `docs/size-budget.md` — binary size measurements and decisions
- `docs/webidl.md` — DOM API surface verification strategy
