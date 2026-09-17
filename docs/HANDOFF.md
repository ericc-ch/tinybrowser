# Handoff (2026-09-17)

State: branch `chase/conformance-loop` in the primary checkout, clean at
`63a5224`, no upstream and no PR. It is based on `chase/dom-events`
(draft PR #16, unmerged), so landing needs a rebase after #16 or an
explicit stacked PR. Gates green at `63a5224`: `tools/ub lint`,
`cargo test --workspace` (29 binaries), release binary 5,870,832 bytes.

Done:

- WPT loop: `tools/wpt/score` + `retest` with `--save-report`; uv now owns
  the venv (`007796f`), docs in `tools/wpt/README.md`. The `--` separator
  is stripped by `tools/wpt/run`; `--test-types` must stay last.
- rquickjs 0.13.0 (`fcd94be`); unsafe exception in `AGENTS.md` (`6e091c1`);
  `tools/ub` lint/miri/valgrind (`c86add1`) and its land-pr gate (`ca6a0f1`).
- `window.postMessage` per html.spec.whatwg.org/multipage/webmessaging.html
  (`199be80`): targetOrigin, structured clone, transfer, async task, zero-arg
  TypeError. Handler attributes are engine-invoked; the shim must not also
  register them as listeners (`dd3baaf`).
- Scores (`docs/progress.md`): webmessaging 41.2% (56/136), FileAPI 48.5%.
  Reports: `/tmp/wpt-suite/baseline/webmessaging4.json`, `wm-ports3.json`.

In flight:

- Cross-frame `postMessage` — the requested next slice, not started. Parent
  and child frames are separate realms, so the JS-side clone cannot cross:
  this needs the Rust-owned payload transport designed for workers (tagged
  schema, size/depth caps, port endpoint ids). Blocked tests: `window[0]`,
  `contentWindow`, `event.source`/`origin`, `postMessage_*xorigin*`,
  `with-ports/017..021`, `without-ports/016..021`.

Next:

1. Implement `contentWindow`/WindowProxy on `HTMLIFrameElement`, indexed
   frame access (`window[0]`, `frames.length`), and `parent`/`top` per
   html.spec.whatwg.org/multipage/window-object.html#the-windowproxy-exotic-object,
   with the cross-realm payload transport and a task queued on the child
   document's task source.
2. Verify: `nix develop --command ./tools/wpt/score webmessaging/with-ports/
   webmessaging/without-ports/ --save-report /tmp/wpt-suite/baseline/wm-frames.json
   -- --exclude=worker --processes 8 --fully-parallel`, then the land-pr gates.
3. Land: rebase after #16 or open a stacked PR with the user's call; commits
   are unsigned while the keyring is locked (`tools/ub` signing note), so
   re-sign before merge.

Decisions made:

- Same-window `postMessage` lives in the JS shim; Rust owns cross-realm
  queues, payload schema, and port endpoints. Do not grow the JS clone into
  a cross-realm one.
- Event handler attributes are invoked by the engine after the listener
  list; shims only define properties the engine reads.
- Cross-frame shape follows Chromium's WindowProxy split: a stable outer
  proxy object per frame (reused across navigations, `frames[0] === frames[0]`)
  holding a frame identity, separate from the inner global
  (bindings/core/v8/local_window_proxy.*, remote_window_proxy.*).
- Serialize in the sender realm before any hop; version the payload format
  (Chromium's kWireFormatVersion); re-check the origin match at delivery,
  not only at call; dispatch `messageerror` when decode fails
  (local_frame.cc DispatchMessageEventWithOriginCheck, PostMessageEvent::Run).

Gotchas:

- One WPT run at a time: the wrapper locks the shared venv; kill stale
  watchers before rerunning.
- Scratch WPT files must be deleted before committing; never leave them in
  `third_party/wpt`.
- `/tmp` baselines are per-run; `score --save-report` before a retest.
