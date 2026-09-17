# Handoff (2026-09-17)

State: branch `chase/cross-frame-postmessage` (not pushed, based on `main`
at `8478208`), clean tree apart from the user's `AGENTS.md` wording change
and an untracked `.zed/`. Gates green: `tools/ub lint`, `cargo test
--workspace` (29 binaries), release binary 5,987,760 bytes. Webmessaging is
77.9% (106/136), up from 47.1% (64/136) on `main`.

Done in this branch:

- Cross-frame `postMessage` per web-messaging.html: `WindowProxy` per frame
  per realm (stable across navigations, shared identity with
  `contentWindow`/`window[i]`/`event.source`), `parent`/`top`/`frames`/
  `length`/`closed`, Blink's `[CrossOrigin]` member policy (same-origin
  forwards through the target realm; cross-origin throws SecurityError),
  and `messageerror`. Delivery re-checks the target origin at task time.
- The Rust-owned transport: sender-realm serialization into a versioned
  `tb1:` payload, target-realm decode, `messageerror` on decode failure,
  depth/type fidelity (Date, RegExp, Error, Map, Set, Blob, File, buffers,
  views, boxed primitives, cycles and sharing), `DataCloneError` at send.
- `MessagePort` endpoints moved into Rust (`crates/renderer/src/messaging.rs`):
  queue, entanglement, transfer in transit, enabled/disabled state, close
  events, and delivery that follows a port across realms. `structuredClone`
  and both `postMessage` paths use the same encoder/decoder.
- Child frames: synchronous browsing-context creation at `iframe` insertion,
  deferred realm materialization (QuickJS forbids entering a realm while
  another executes; proxy writes made meanwhile are flushed), `src`
  navigation for http(s)/data:/about:blank/javascript:/blob:, iframe `load`
  events, and the parent's `load` event waits for child frames
  (delay-the-load-event). Frames drain round-robin so cross-frame task order
  matches queue order.
- Event handler properties (`element.onload`, `window.onmessage`) live in
  the World rather than on the wrapper, so a collected wrapper cannot lose
  them; handler content attributes compile into the same slots, and a
  `body` element's window handler attributes register on the window.
- Parser handoff keeps document/wrapper identity (`World::set_document` no
  longer forgets wrappers for the same document), so `iframe.onload = f`
  set during a script survives to the load event.

Reports: `/tmp/wpt-suite/baseline/webmessaging-frames4.json` (106/136),
baseline before the work `/tmp/wpt-suite/baseline/webmessaging-current.json`
(64/136).

Next, in order:

1. BroadcastChannel: origin-scoped channel table keyed by name+origin, same
   payload transport and port endpoints, `onmessage`/`onmessageerror`,
   close semantics. Unblocks 12 webmessaging tests plus
   `MessageEvent-trusted.any`.
2. Workers: agent-context split, dedicated worker thread, placement; then
   `postMessage` to/from workers rides the same transport. Unblocks the four
   worker tests and `postMessage_CryptoKey_insecure`-style gaps remain
   separate.
3. `location.reload()` and "fully active" documents, for
   `postMessage-to-target-not-fully-active-on-reload`.
4. The remote-context helper tests (`close-event/*`, `multi-globals/*`)
   need `document.write` window replacement and `frames[i]` cross-origin
   helper access; they are not messaging bugs.
5. Canvas 2D + ImageData, which gates `with/without-ports/011`,
   `without-ports/028`, and `postMessage_cross_domain_image_transfer_2d`.

Accepted gaps (honest failures, not baselines):

- Realm teardown: calling a platform method from a dropped realm throws
  `InternalError: missing JS world`
  (`multi-globals/messageport-current.html`).
- `postMessage(message, targetOrigin)` on a frame whose realm has not been
  materialized yet defers property reads on `contentWindow`; writes are
  stored and flushed, reads return undefined until then.
- `document.location` on child frames is null, and `location.reload()` is
  not implemented.

Gotchas:

- One WPT run at a time: the wrapper locks the shared venv.
- Scratch WPT files must be deleted before committing; never leave them in
  `third_party/wpt`.
- `/tmp` baselines are per-run; `score --save-report` before a retest.
- Commits are unsigned while the keyring is locked; re-sign before merge.
- `set_document`'s wrapper-retention rule is load-bearing for handler
  properties; do not reintroduce `drop_active_document` on the parser path.
