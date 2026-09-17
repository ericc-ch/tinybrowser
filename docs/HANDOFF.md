# Handoff (2026-09-17)

State: branch `chase/cross-frame-postmessage` (not pushed), working tree has
the cross-frame work from `6e1df39` plus the review fixes and the bindings
split. The user's `AGENTS.md` wording change and an untracked `.zed/` are
untouched. Gates green: `tools/ub lint` (clippy), `cargo test --workspace`
(29 binaries), `tools/js/check` (8 embedded scripts), release binary
5,998,000 bytes. Webmessaging is 77.9% (106/136); the refactor run is
`/tmp/wpt-suite/baseline/webmessaging-refactor.json`, identical to the
pre-refactor fix run.

Done in this slice:

- Cross-frame `postMessage` per web-messaging.html: WindowProxy per frame per
  realm, stable across navigations, shared identity with `contentWindow`/
  `window[i]`/`event.source`; Blink's `[CrossOrigin]` member policy;
  delivery-time target-origin re-check; `messageerror`.
- Rust-owned transport (`messaging.rs`): versioned `tb1:` payloads, sender
  serialize / receiver decode, `MessagePort` endpoints with queue,
  entanglement, transfer-in-transit, close events.
- Child frames: sync browsing-context creation, deferred realm
  materialization, `src` navigation (http(s)/data:/about:blank/javascript:/
  blob:), iframe load events, delay-the-load-event, round-robin drains.
- Review fixes:
  - Port endpoints carry an intended recipient, set when a carrying message
    is delivered; `adopt` rejects other realms and `post/start/close/peer`
    require ownership. A child frame can no longer brute-force an endpoint id
    to close or steal another frame's port (probe-verified: the foreign close
    is rejected and the port still delivers).
  - A stale frame-load response can no longer replace a newer document
    (`reset_js_realm` bumps the frame-load sequence; synchronous
    about:blank/data:/javascript: navigations invalidate in-flight dials).
  - `is_initial_blank` is a document flag, not URL equality:
    `<iframe src="/same-as-parent">` loads instead of being skipped.
  - Frame load marks move with the parent document; `reset_js_realm` clears
    them together with the pending-child counter.
  - Same-document iframe moves reorder the frame tree (DOM mutation serial
    drives the rescan).
  - Closed endpoints are removed from the table (no unbounded growth), a
    dropped message disentangles its ports and fires `close`, and a decoded
    failure discards the transferred ports.
  - `data:` URLs: case-insensitive `;base64`, forgiving-base64 failure leaves
    the frame on its document.
  - Clone fidelity: Promise/WeakMap/WeakSet/WeakRef/Proxy-ish internal-slot
    objects throw DataCloneError instead of cloning as `{}`; detached buffers
    throw DataCloneError; `DOMException` is serializable; `__proto__` decodes
    as an own property; `__tbIsError` is not `Symbol.toStringTag`-spoofable.
  - MessagePort: options-dictionary overload, transfers consumed on a closed
    port, `onmessage = null` still enables the queue, `close` fires after the
    carrying message dispatch.
  - Handler properties: an explicit clear wins over the content attribute
    until the attribute changes again; `DOMException.prototype[@@toStringTag]`
    is set; `location` stringifies to its URL; duplicate `javascript:` frame
    evaluation is gone.
- Structure: `js/bindings.rs` (8,060 lines) split into
  `js/bindings/{mod,node,attributes,mutation,parsing,collections,exceptions,
  webidl,clone,messaging,window,document,focus}.rs`; `node.rs` (2,793) keeps
  the class because `#[rquickjs::methods]` emits one `MethodImplementor` impl
  per type. Embedded JS moved to `js/scripts/*.js` with `include_str!`;
  `tools/js/check` runs `node --check` over them.

Deferred review findings (understood, not baselines):

- `FileList` is not serializable yet (DOMException is); `structuredClone`
  refuses `SharedArrayBuffer` rather than sharing it.
- Proxies are not detected as uncloneable (no JS-visible Proxy brand); a
  proxy over an ordinary object clones its trap results.
- WindowProxy `getOwnPropertyDescriptor` still returns undefined, and a
  cross-origin `location` member throws instead of returning a restricted
  Location.
- Brand symbols are `Symbol.for`, so a page can forge them; with the new
  ownership checks the impact is self-corruption only.
- `frame-load` is a new wasm WIT variant; no host run in this repo yet.

Next, in order:

1. BroadcastChannel (origin-scoped channel table, same transport; unblocks
   12 webmessaging tests plus `MessageEvent-trusted.any`).
2. Workers (agent-context split, placement); four webmessaging tests.
3. `location.reload()` and "fully active", `navigator.userActivation`,
   `navigator`, WebCrypto (`postMessage_CryptoKey_insecure`).
4. Remote-context helper tests (`close-event/*`, `multi-globals/*`) need
   `document.write` window replacement.
5. Canvas 2D + ImageData (`with/without-ports/011`, `without-ports/028`,
   `postMessage_cross_domain_image_transfer_2d`).

Gotchas:

- One WPT run at a time: the wrapper locks the shared venv.
- `tools/js/check` requires node (present in `nix develop`).
- The generated `bindings/` split keeps `use super::...` re-exports in
  `mod.rs`; a new sibling needs its module declared before the macro and its
  name re-exported, or imports fail.
- Scratch WPT files must be deleted before committing; commits are unsigned
  while the keyring is locked, so re-sign before merge.
