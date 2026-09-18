# Handoff (2026-09-19)

State: branch `chase/wpt-grind` (not pushed) on top of `main` at `00c5940`.
Tip `fbf0a26`. `webstorage/` scores 47/54 (87.0%); the new
`webmessaging/broadcastchannel/` group scores 5/12 (41.7%). `tools/ub lint`,
`cargo test --workspace` (34 suites), and Playwright 7/7 are green at the tip.

Done (this branch):

- `daf2f95 feat(browser): cross-tab postMessage and window.opener`. Tab
  actors register assignment -> tab; `ToRenderer::WindowMessage` routes
  messages; `window.opener` is a remote-window object.
- `f4ced46 feat(browser): session copies, named windows, remote session
  reads`. A `StorageSeed` from the opener is applied before the first
  document mounts; named windows reuse one proxy; `opener.sessionStorage` is
  a live remote read.
- `fbf0a26 feat(renderer): BroadcastChannel fan-out and window.origin`. A
  browser `BroadcastBus` fans out to every renderer; detached iframes neither
  post nor receive; `PROTOCOL_VERSION` 8.

Verification for each: `tools/wpt/score <group>/ -- --exclude=worker
--processes 4` and the gates above; scores are recorded in `docs/progress.md`.

In flight: nothing uncommitted.

Next:

1. `webstorage/`'s remaining 7 failures need engine work, not storage:
   `document-domain` and `event_no_duplicates` need a child `Window` to
   exist synchronously after `appendChild` (today the realm is materialized
   after the task, so `contentWindow.addEventListener`/`frames[0]` are
   undefined inside the same script); `localstorage-cross-origin-iframe`
   and the three partitioned tests need storage partitioning plus the
   cross-origin dispatcher.
2. `webmessaging/broadcastchannel/`: `basics` needs event-handler
   properties (`onmessage`) to keep their listener-list position instead of
   running after `addEventListener` listeners; `ordering` times out;
   `workers`/`service-worker`/`blobs` need workers; `opaque-origin` and
   `cross-partition` need workers/partitioning.
3. `url/` at 14.3%: the failures are the `<a>` URL-decomposition attributes
   (`protocol`, `origin`, `host`, `pathname`, `search`, `hash`, passwords).
   `href` exists on the shared element class; the rest are missing. Files
   are large (one up to 736 subtests) and must pass whole.
4. Wider probes (`domparsing/`, whole directories) can run long and hang;
   score one directory at a time and keep `--save-report` for retests.

Decisions made:

- Broadcast messages and storage events share the browser-fan-out shape;
  the source assignment skips only its own channel/window.
- Detachment is decided by the iframe container's connectedness, so
  `remove()` in the same task is honored before frame reconciliation.
- `window.origin` serializes to `"null"` for opaque origins.

Gotchas:

- The shared WPT checkout lives in the primary worktree
  (`/home/erickc/projects/tinybrowser/third_party/wpt`); `tools/wpt/run`
  falls back to it. Scratch tests go there and must be deleted before
  scoring.
- One WPT run at a time. `webmessaging/broadcastchannel` takes ~2.5 min;
  `domparsing/` and `url/` can exceed 25 min and kill the report.
- Wire changes need a `PROTOCOL_VERSION` bump and rebuilt debug/release
  binaries; `tools/ship` produces the shipping artifact.
