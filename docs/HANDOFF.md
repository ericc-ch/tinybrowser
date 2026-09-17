# Handoff (2026-09-17)

State: `main` at `01f175b`, clean, one worktree. #16 and #18 merged; the
conformance loop and its slices are in. Gates green at `dd9ca8b`: clippy,
`cargo test --workspace` (29 binaries), release binary 5,871,280 bytes.

Done (merged):

- WPT loop: `tools/wpt/score` + `retest`, uv-owned venv (`tools/wpt/run`),
  docs in `tools/wpt/README.md`. `--test-types` must stay last; the `--`
  separator is stripped.
- rquickjs 0.13.0; unsafe exception in `AGENTS.md`; `tools/ub`
  lint/miri/valgrind with the land-pr gate.
- Messaging: `window.postMessage` per web-messaging.html (targetOrigin,
  clone, transfer, async task, options overload), trusted message events
  (`__tbDispatchTrusted`), MessageEvent/MessagePort/MessageChannel,
  structuredClone identity/sharing/boxed primitives, engine-invoked
  handler attributes that respect stop flags.
- Scores in `docs/progress.md`: webmessaging 41.2% (56/136), FileAPI 48.5%.
  Reports: `/tmp/wpt-suite/baseline/webmessaging4.json`, `wm-ports3.json`.

Next:

1. Cross-frame `postMessage`: `contentWindow`/WindowProxy, indexed frame
   access (`window[0]`), and the Rust-owned cross-realm payload transport
   (parent and child frames are separate realms, so the JS clone cannot
   cross). Follow Chromium's shape: a stable outer proxy per frame reused
   across navigations, serialize in the sender realm before any hop,
   version the payload, re-check the origin match at delivery, dispatch
   `messageerror` on decode failure.
2. Verify with `nix develop --command ./tools/wpt/score
   webmessaging/with-ports/ webmessaging/without-ports/ --save-report FILE
   -- --exclude=worker --processes 8 --fully-parallel`, then the land-pr
   gates.
3. Then workers: agent-context split, dedicated worker thread, placement.

Accepted gaps (tracked on the closed PR #17, body lists them):

- Object URLs are not binary-safe (bytes round-trip through a decoded
  string); needs the response-body path to carry bytes.
- `docs/progress.md` rows lack retained reports; rerun with
  `--save-report` before quoting them.
- Queue: `input.files` null, base64 padding, `location.origin`,
  `TINYBROWSER_WPT_VENV`, `retest --save-report`, stale reports, Error
  subclass fidelity.

Gotchas:

- Commits are unsigned while the keyring is locked; re-sign before merge.
- One WPT run at a time: the wrapper locks the shared venv.
- Never leave scratch tests in `third_party/wpt`.
