# Handoff (2026-09-19)

State: `main` clean and green at `b0d2863`; `webstorage/` scores 42/54
(77.8%). `tools/ub lint`, `cargo test --workspace` (34 suites), and
Playwright 7/7 are green. `window.open` step 1 shipped; step 2 (cross-tab
messaging) is next and unstarted.

Done:

- `5808bad feat(storage): localStorage, sessionStorage, and storage events`:
  browser-owned `localStorage` per origin persisted next to `cookies` with a
  5 MiB quota; renderer-owned `sessionStorage`; legacy `Storage` Proxy;
  `StorageEvent`; cross-renderer broadcast; `PROTOCOL_VERSION` 5. Verified by
  `tools/wpt/score webstorage/ -- --exclude=worker --processes 4` (41/54).
- `9632e92 progress: score webstorage at 75.9% (41/54)`.
- `b0d2863 feat(browser): window.open creates a tab, window.close closes it`:
  renderer links carry a `BrowserHandle`; `window.open` resolves its URL in
  the calling realm, the browser opens a tab (about:blank first, navigation
  in the background), and the renderer gets a per-tab proxy with `close()`.
  `PROTOCOL_VERSION` 6. Verified by `tools/wpt/score webstorage/` (42/54) and
  `event_local_window_open_oldvalue.html` passing.
- Also in `5808bad`: subframe `Document.URL`/`documentURI` no longer return
  `about:blank`; parser-authored `<body onstorage>` forwards to the window.
- `.cargo/config.toml` uses GNU bfd for dev/test links (this rustc defaults
  to rust-lld, which rejects a rustc 1.98.1 DWARF relocation); release bins
  still use lld via `build.rs`.

In flight: nothing uncommitted.

Next:

1. Step 2: cross-tab `postMessage` and `window.opener`. Track the calling tab
   in the browser (actors register assignment -> tab) so `open_window` knows
   the opener, add a browser-routed `ToRenderer::WindowMessage`, and let the
   new tab's renderer expose `globalThis.opener` as a remote window object
   whose `postMessage` encodes with the existing `__tbEncode` payload.
   Unlocks `event_session_window_open_scope` and `storage_local_window_open`.
2. Step 3: session snapshot (`Command::SessionSnapshot`) seeded into the new
   tab before navigation, and a (opener tab, name) registry for named
   windows; `opener.sessionStorage` can serve the open-time snapshot.
   Unlocks `storage_session_window_open`, `storage_session_window_reopen`.
3. Still blocked on engine work, not window.open: `document-domain` and
   `event_no_duplicates` (synchronous child Window), window named access
   (`testDiv`), `BroadcastChannel`, storage partitioning.

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
