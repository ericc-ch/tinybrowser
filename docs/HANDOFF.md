# Handoff (2026-09-19)

State: `main` clean and green at `9632e92`; `webstorage/` scores 41/54
(75.9%). `tools/ub lint`, `cargo test --workspace` (34 suites), and
Playwright 7/7 are green. The next cut is `window.open`; nothing is in flight.

Done:

- `5808bad feat(storage): localStorage, sessionStorage, and storage events`:
  browser-owned `localStorage` per origin persisted next to `cookies` with a
  5 MiB quota; renderer-owned `sessionStorage`; legacy `Storage` Proxy;
  `StorageEvent`; cross-renderer broadcast; `PROTOCOL_VERSION` 5. Verified by
  `tools/wpt/score webstorage/ -- --exclude=worker --processes 4` (41/54).
- `9632e92 progress: score webstorage at 75.9% (41/54)`.
- Also in `5808bad`: subframe `Document.URL`/`documentURI` no longer return
  `about:blank`; parser-authored `<body onstorage>` forwards to the window.
- `.cargo/config.toml` uses GNU bfd for dev/test links (this rustc defaults
  to rust-lld, which rejects a rustc 1.98.1 DWARF relocation); release bins
  still use lld via `build.rs`.

In flight: nothing uncommitted; `window.open` not started.

Next:

1. Step 1: `window.open` creates a tab and returns a proxy with `close()`
   (passes `event_local_window_open_oldvalue`). The link forwards
   `ServiceCall::WindowOpen` to the owning tab actor through the
   per-assignment `EventSubscribers` with the service id; `TabTask::spawn`
   gains a `BrowserHandle`; the actor replies `ToRenderer::WindowOpened
   { id, tab }`, which `ChannelServices` delivers to the blocked caller.
2. Step 2: cross-tab `postMessage` via `ToRenderer::WindowMessage`, plus
   `window.opener` as a remote window object. Unlocks
   `event_session_window_open_scope`, `storage_local_window_open`.
3. Step 3: session snapshot (`Command::SessionSnapshot`) seeded into the new
   tab before navigation, and a (opener tab, name) registry for named
   windows; `opener.sessionStorage` can serve the open-time snapshot.
   Unlocks `storage_session_window_open`, `storage_session_window_reopen`.

Decisions made:

- `localStorage` lives in the browser (one area, all tabs); `sessionStorage`
  stays per renderer and top-level context.
- Storage events broadcast to all renderers; only the source window of the
  mutating assignment is excluded; saturation drops instead of blocking.
- Lone surrogates cross the UTF-8 seam JSON-escaped from the JS shim.

Gotchas:

- One WPT run at a time; scratch WPT files must be deleted before commit.
- Child windows materialize after the current task, so synchronous
  `contentWindow` members are undefined (`document-domain` and
  `event_no_duplicates` need an engine fix, not a storage fix).
- The 7 remaining `webstorage/` failures need `window.open`; the two
  `*-partitioned.sub.html` tests may also need storage partitioning.
- Wire changes need a `PROTOCOL_VERSION` bump and rebuilt binaries;
  `tools/ship` produces the shipping artifact.
