# Handoff (2026-09-21)

Goal: Land the browser-ownership refactor (local/session storage out of `NetworkSession`, assignment-scoped broker, split renderer host capabilities, one shared ordered `StorageArea`) with no `webstorage/` WPT regression from the recorded 87.0%.

Plan:

1. Unify storage mechanics in new `crates/webstorage` for native local/session + WASM.
2. Replace `NetworkSession` with private `BrowserContext` + `StoragePartition`; keep explicit `Browser::open_in` / `open_in_with_network`, no builder.
3. Introduce assignment-scoped broker (`AssignmentContext`, `RendererHostBroker` routing in `link.rs`) and per-tab `TabNetworkHandle`.
4. Split renderer host into network/storage/messaging/browsing-context capabilities over one IPC channel; move authoritative `sessionStorage` browser-side per `TabId`; delete seed/remote-read machinery.
5. Remove obsolete types, run `tools/check`, `cargo test --workspace`, CDP/Playwright, targeted + scored WPT, release-size check; update only permitted fields in `docs/progress.md`.

State: Branch `main` at `e4b3bc3`, tree dirty (refactor uncommitted). `tools/check` green (clippy + 24 JS files). `cargo test --workspace` green. `cargo test --workspace --no-run` green. WPT `webstorage/` RED: 42 ran as expected, 6 crashed, 5 timed out, 6 subtests unexpected. `docs/progress.md:291` still records `webstorage/ 87.0%` and must not be touched until green.

Done:

- None this session (refactor is uncommitted, so nothing shippable to cite yet).

In flight:

- Uncommitted ownership refactor. New: `crates/browser/src/context.rs`, `crates/webstorage/`. Modified: `Cargo.toml`, `Cargo.lock`, `crates/browser/*` (`actor.rs`, `broadcast.rs`, `browser.rs`, `child/mod.rs`, `child/session.rs`, `lib.rs`, `link.rs`, `manager.rs`, `network.rs`, `storage.rs`, `store.rs`, `wire/mod.rs`), `crates/renderer/*` (`protocol.rs`, `lib.rs`, `engine.rs`, `document/mod.rs`, `storage.rs`, `js/mod.rs`, `js/bindings/mod.rs`, `js/world.rs`, `js/scripts/web/messaging.js`), `crates/wasm/*`, `src/main.rs`. See `git status --short` / `git diff --stat` from `e4b3bc3`.
- Blocker repro: `nix develop --command tools/wpt/run webstorage/storage_setitem.window.html` ends `CRASH [expected OK]`. Full `nix develop --command tools/wpt/run webstorage/` shows quota-setitem crashes (`storage_setitem`, `storage_local_setitem_quotaexceedederr`, `storage_session_setitem_quotaexceedederr`, quota-independent tests), `event_no_duplicates.html` session-event timeouts, plus partitioned/cross-origin timeouts and one `localstorage-share-data-unrelated-origins` failure. Prior storage baseline is commit `5808bad` (its message claims 41/54 + Playwright 7/7).

Next:

1. Debug the quota `setItem` crash first (suspects: large-value IPC control path around `wire/channel.rs:38` 8 MiB cap, quota-error reply path, session-event fan-out in `link.rs`).
2. Fix `event_no_duplicates` session no-op/timeout behavior.
3. Re-run single-file WPT, then full `webstorage/`, then `tools/check`, `cargo test --workspace`, CDP/Playwright, scored WPT + size check.
4. Update `docs/progress.md` latest fields only, then commit with design decisions in the message.

Decisions made:

- Complete ownership scope, not a minimal rename.
- Explicit constructors over `BrowserBuilder` (too few options to justify it).
- `BrowserContext` (one live profile world) vs `StoragePartition` (one site-data bucket: network jar, localStorage, messaging hub).
- Split renderer host capabilities logically; keep one physical IPC channel.
- Share one ordered `StorageArea` now; native `BTreeMap` vs WASM `Vec` ordering already disagreed.
- Browser-owned per-`TabId` session namespaces; `window.open` copies before navigation; renderer stays disposable.
- Storage mutations pure; broker/event hub (`RendererEventHub`, `ContextEvent`) delivers events; grouped wire enums (`Network`/`Storage`/`BrowsingContext`/`Messaging`).

Gotchas:

- Must run under `nix develop` (OpenSSL/pkg-config, stylo python3, WPT venv). Bare `cargo`/`python3` fail outside it.
- `cargo test` alone is insufficient here; spec regressions only count via `tools/wpt/run`.
- Storage cargo tests were deleted per repo rule (no spec assertions in cargo tests); use WPT for storage behavior.
- Clippy denies `too_many_lines` and `enum-variant-names`; storage router was split into `StorageRouter` and `StorageCall::{Get,Keys,Set,Remove,Clear}` for this.
- Format touched Rust files only (`rustfmt --edition 2024`); workspace-wide format drifts on untouched files.
- `HostNotice::StorageEvent.target=None` means local broadcast; `Some(assignment)` means targeted session event (`wire/mod.rs:116`, `child/session.rs:142`, `link.rs` subscribe/publish paths).
- WPT report temp files from the failing run were 0-byte; rely on console output + `tools/wpt/retest`.
- WASM session state is keyed by component owner id and dropped in `WasmServices::drop`.

Read this conversation via @opencode (verified against local CLI `--help` plus `session list`):

```sh
cd /home/erickc/projects/tinybrowser
opencode session list --max-count 5
opencode session export ses_f3cceada4ffeAbiQnBnawnMYV5 --sanitize | head -c 4000
opencode run --session ses_f3cceada4ffeAbiQnBnawnMYV5 "Read docs/HANDOFF.md and continue Next step 1"
```

- `session list` shows this session as `Local storage in network session confusion`.
- `session export` prints the transcript JSON; `--sanitize` redacts sensitive data.
- `run --session` continues this session non-interactively; add `--fork` to branch instead of appending.
- Durable resume path is still this file plus `git log --oneline -8`, `git status --short`, `git diff --stat`.
