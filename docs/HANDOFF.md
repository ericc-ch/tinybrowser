# Handoff (2026-09-09)

State: branch `feat/named-profile-daemon` (from `e050c38`). Named-profile daemon, PageActor, CDP CLI, WebDriver adapter, and WPT temp profiles are on this branch. rustc 1.98.0.

Done:

- Named-profile daemon, `PageActor`/`BrowserHandle`, `ProfileStore` cookies, `cdp` crate, CLI over CDP, WebDriver as `BrowserHandle` adapter, WebIDL branding + weak wrappers, WPT temp XDG profile. Proof: `cargo clippy --workspace --all-targets --offline -- -D warnings`; `cargo test --workspace --offline`; stripped x86_64 `target/release/tinybrowser` 3,398,512 B (`page_probe` 3,013,552 B). Slice 12 review: no must-fixes.

In flight:

- First green testharness file through `./tools/wpt/run` ([ADR 0008](adrs/0008-wpt-via-webdriver.md)).
- Should-fixes: WPT `stop` rmtree while process still alive; temp tree leak on start failure; `createTarget` page leak if `open_url` fails; PID 1 cannot register; empty-lock tests.

Next:

1. First testharness file through `./tools/wpt/run`.
2. Optional should-fixes above.

Decisions made:

- Daemon/CDP/WebDriver adapter: [ADR 0009](adrs/0009-named-profile-daemon.md). Browser/`PageActor`/`NetworkSession`: [ADR 0010](adrs/0010-page-actor-ownership.md). WPT via WebDriver + temp profile: [ADR 0008](adrs/0008-wpt-via-webdriver.md).

Gotchas:

- `Browser.close` stops `cdp::serve`. Startup lock treats empty/unparseable pid as in-progress (2s) and pid ≤ 1 as stale. WebDriver script timeout is still two clocks. Host objects are one `JsNode` class plus JS branding. Wrapper cache uses the page `WeakRef` constructor.
