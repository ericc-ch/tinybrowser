# Handoff (2026-09-19)

State: branch `chase/wpt-grind` (not pushed), tip `7bb3da2`, clean. Scores on
this branch: `webstorage/` 47/54 (87.0%), `url/` 14/49 (28.6%),
`webmessaging/` 106/124 (85.5%, broadcastchannel excluded),
`webmessaging/broadcastchannel/` 5/12 (41.7%), `focus/` 3/41 (7.3%, was
scored and unchanged). `tools/ub lint`, `cargo test --workspace` (34 suites),
and Playwright 7/7 are green at `1a95b4e`.

Done (this branch):

- `daf2f95` cross-tab `postMessage` + `window.opener`; actors register
  assignment -> tab.
- `f4ced46` session copy before the first document mount, named windows,
  live remote session reads.
- `fbf0a26` `BroadcastChannel` fan-out, `window.origin`, detached-iframe
  semantics; `PROTOCOL_VERSION` 8.
- `1a95b4e` `<a>`/`<area>` URL decomposition and `document.baseURI` backed
  by the `url` crate; base-relative resolution first (`http:foo.com`).
- Score commits `b505cb9`, `7bb3da2`; scores recorded in `docs/progress.md`.

Next (ordered by expected file yield):

1. `focus/` needs a cross-frame focus subsystem: `window.focus()`,
   `document.hasFocus()`, and the ancestor `activeElement` chain when an
   iframe's element is focused or loses focus. 30 of 41 files time out
   waiting for that; the two `focus-element-crash` files pass, event
   plumbing exists.
2. `webmessaging/` leftovers are each a distinct subsystem: workers
   (`Worker`/`SharedWorker`), canvas 2D (`getContext`/`getImageData` for the
   ImageData clone tests), cross-origin globals with `document.domain`,
   MessagePort close across documents and BFCache, user activation, and
   JS-initiated main-frame navigation (`location.reload`).
3. `url/` leftovers are upstream/crate-shaped: the `url` crate validates
   punycode labels that browsers accept (11 origin cases), lone surrogates
   cannot cross the UTF-8 attribute seam (2), and opaque-path spaces
   (`non-special:opaque  ?hi` -> `%20`) are not encoded (24). Empty
   `search`/`hash` (`?`/`:#`) and file drive-letter `|` -> `:` are fixable
   post-processing if `url/` is worth more than the other groups.
4. `webstorage/` needs storage partitioning (3 files), the cross-origin
   dispatcher (1), and synchronous child-`Window` materialization (2).
5. `FileAPI/` (32/68, 47.1%) clusters on Blob URLs: the renderer mints
   `blob:tinybrowser/<id>`, an opaque-origin shape, while the spec embeds the
   creator's origin and a UUID; the hand-written JS `URL` class has no
   `host`/`port`/`pathname` getters or blob-origin handling; and
   `window.open`/iframe loads of a `blob:` URL need a browser-side blob
   registry (the object-URL table is renderer-local). `Blob-methods-*`
   failures ("emptyDocumentIframe is not defined") look like page-setup
   fallout, and `idlharness`/workers/`historical.https` need workers or
   WebIDL introspection.
6. `domparsing/` and whole-tree probes hang past 25 minutes; score subsets
   (or one directory at a time) and keep `--save-report`.

Decisions made:

- Broadcast fan-out and storage events share one best-effort browser bus;
  the posting assignment skips only its own channel/window.
- Anchor decomposition lives in `brands.js` accessors over Rust
  `url_parts` hooks, not in the shared element prototype.
- Base-relative parse comes first so special schemes without slashes join
  the base.

Gotchas:

- The shared WPT checkout is the primary worktree's
  `third_party/wpt`; scratch tests go there and must be deleted.
- Killed WPT runs leave orphan `tinybrowser renderer` processes and a stale
  lock in `~/.cache/tinybrowser/`; kill them and rerun with
  `TINYBROWSER_WPT_NO_LOCK=1` when alone.
- `tests/wpt/metadata` is unchanged; scores are file-level, so partial
  subtest wins do not move a group until a whole file passes.
