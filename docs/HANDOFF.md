# Handoff (2026-09-19)

State: branch `chase/wpt-grind` (not pushed), tip `4cfbca2`, clean. Scores on
this branch: `webstorage/` 47/54 (87.0%), `url/` 25/49 (51.0%),
`webmessaging/` 106/124 (85.5%, broadcastchannel excluded),
`webmessaging/broadcastchannel/` 5/12 (41.7%), `focus/` 3/41 (7.3%),
`domparsing/` 21/74 (28.4%), `FileAPI/` 32/68 (47.1%). Shipping binary
8,300,088 bytes (cap 10,485,760). CDP `--all` re-run post-fix: PASS 33
(unchanged), UNSUPPORTED_METHOD 1, TIMEOUT 423 (+4), PROTOCOL_FAILURE 505
(-4); the shift is input-dispatch tests proceeding past the fixed envelope
bug into waits for unimplemented async domains (`Debugger.paused`,
`lifecycleEvent`, late `styleSheetAdded`), verified in isolation.
(one intentional `Domain.NotExistingCommand` test). `tools/ub lint`,
`cargo test --workspace` (34 suites), and `tools/ship` are green at
`4cfbca2`.

Done (this branch):

- `daf2f95` cross-tab `postMessage` + `window.opener`; actors register
  assignment -> tab.
- `f4ced46` session copy before the first document mount, named windows,
  live remote session reads.
- `fbf0a26` `BroadcastChannel` fan-out, `window.origin`, detached-iframe
  semantics; `PROTOCOL_VERSION` 8.
- `1a95b4e` `<a>`/`<area>` URL decomposition and `document.baseURI` backed
  by the `url` crate; base-relative resolution first.
- `258bda8` full JS `URL` IDL (all getters/setters, parsing `href`, live
  `searchParams`, USVString conversion) and browser-grade component writes
  via `url::quirks` plus tab/newline stripping.
- `a8096aa` DOMParser documents take the constructing realm's URL, parse
  with scripting disabled, and throw `TypeError` for a bad enum. The native
  class gains a JS wrapper (`scripts/parsing/dom_parser_ctor.js`) because
  QuickJS runs methods in the object's realm, which the native class cannot
  observe; `Parsed` grew a script-document URL.

url/ remains (24 files), by cluster:

1. IDNA strictness (crate): `IdnaTestV2.any.html` 1912/2671,
   `toascii.window.html` 721/784, `host = 'xn--'` cases, origin files.
2. Opaque-path spaces (crate): `non-special:opaque  ?hi` -> expected
   `%20`; `url-constructor?exclude` 712/737, two `urlsearchparams-delete`
   cases.
3. File URL parsing (crate): drive letters `file:///w|/m`, backslash
   pathname cases (`url-setters?include=file` 19/22,
   `url-setters-a-area?include=file` 37/43), `a-element?include=file`
   98/138, `url-constructor?include=file` 97/137.
4. Ours but not yet done: `urlencoded-parser.any.html` 30/105,
   `percent-encoding.window.html` 9/17, `idlharness` 38/77,
   `urlsearchparams-constructor` 22/27 (needs `FormData`, DOMException
   enumeration, surrogate pairs in iterables), `data-uri-fragment.html`,
   two timeouts (`failure.html`, `javascript-urls.window.html`).

Decision: do not vendor/fork `url` yet. Upstream `main` has unreleased
fixes (#1127 file drive letters, #1141 caret percent-encoding) that should
land in 2.5.9; the IDNA laxness is deliberate upstream and would stay ours.
Vendoring recipe if we revisit: `[patch.crates-io] url = { path = "vendor/url" }`
with the crates.io tarball (628 KB, 9 files), patch files outside the tree,
a `tools/vendor-url` updater, `vendor/url` in workspace `exclude`. `url` is
pinned by stylo, so it cannot be dropped even if the renderer moved to ada.

Deferred with a reason: `webmessaging/broadcastchannel/basics.any.html`
(5/7). Two subtests need post-time target snapshots: a channel created during
delivery must not receive the in-flight message, and the message posted
during delivery must not reach it. The renderer iterates a live `Set` at
delivery time (`__tbDeliverBroadcast`), so snapshots must come from
`postMessage` time - either a watermark/realm id through the wire or a
browser-side channel registry keyed by (assignment, channel id).

Next:

1. CDP and WebDriver are the active workstream (the user asked to unblock
   them fully). Current state:
   - CDP `--all` after the method-table campaign: PASS 33,
     `UNSUPPORTED_METHOD` **1** (down from 831; the one left is a test that
     deliberately calls `Domain.NotExistingCommand` and expects an error),
     `PROTOCOL_FAILURE` 509, `TIMEOUT` 419, `MISSING_FIXTURE` 525,
     `HARNESS_UNSUPPORTED` 7. Every CDP method call is now answered; the
     remainder is behavior, not plumbing. `DOM.getDocument`/queries,
     `Input.*`, `CSS.enable` + `getComputedStyleForNode`, `Tracing.end`,
     `Storage.getStorageKey`, and the fixed-shape replies are real; the rest
     of the CSS/DOM/Emulation/Page/Target/Network/Debugger/Input/etc. surface
     answers empty results.
   - The next CDP investment is behavior: workers via
     `Target.attachedToTarget` (59 waits), `Debugger.paused` (40), late
     `CSS.styleSheetAdded` (27), Network request/response events (35),
     `Audits.issueAdded` (16), `Fetch.requestPaused` (13), storage-bucket/
     service-worker/animation events (~35), and the 509 protocol failures
     concentrated in CSS (98), Emulation (73), DOM (64), Page (25).
     `MISSING_FIXTURE` (525) is a runner limitation: the static fixture server
     cannot serve Chromium's `*.test` hosts, HTTPS certificates, or PHP.
   - WebDriver `POST /session/{id}/actions` works for pointer move/down/up/
     cancel, key down/up with text entry, and wheel; `permissions` accepts
     requests but the engine has no permission store. Remaining: touch
     sources, duration interpolation, real permission state, and the engine
     gaps the smoke tests exposed (`window.getSelection`, range inputs,
     canvas selection).
   - Real layout is wired (`78f3c6e`): `tree::BoxNode`/`layout::LayoutBox`
     carry the DOM `NodeId` (element boxes and measured leaves for
     inline-only blocks), `render::layout_boxes` runs style/box/Taffy without
     painting, and `getBoundingClientRect`/`getClientRects`/
     `elementFromPoint`/`elementsFromPoint`/`element_at_point` serve it. The
     virtual stand-in is gone; the Playwright click gate passes. Inline
     elements still fold into measured leaves, so `<span>`/`<a>` rects are
     zeros until inline fragments carry node ids. External stylesheets are
     not mirrored into script geometry yet (inline `<style>` and style
     attributes are).
   - Usability blocker 2, form controls: textarea/input lay out as zero boxes
     (no UA intrinsic size) and the engine does not expose their `value`, so
     Playwright `locator.focus()`/`fill()`/typing fail. The committed
     `tools/playwright/interaction.spec.ts` typing test is `fixme` until UA
     sizing and value reflection land.
   - Usability blocker 3, WebDriver conformance: the vendored WPT checkout
     defines the `wdspec` test type but ships no wdspec executor (no
     `tools/wptrunner/wptrunner/executors/executorwdspec.py`, no pytest in
     `_venv3`), so `wpt run --test-types wdspec` dies with `'NoneType' object
     has no attribute 'test_queue'`. Options: add the upstream wdspec
     executor + pytest to the product/venv, or run the 898 `webdriver/` tests
     through upstream WPT tooling against a product that declares wdspec.
   - Usability blocker 4, feature depth: workers (66 CDP timeouts, and the
     largest WPT pool), then `window.getSelection`/range inputs, W3C touch
     action sources, and action durations.
2. `focus/` needs the cross-frame focus subsystem (`window.focus()`,
   `document.hasFocus()`, ancestor `activeElement` chain, exact focus event
   order); 30 of 41 files time out waiting for it.
3. `webstorage/` endgame: storage partitioning (3 files), cross-origin
   dispatcher (1), synchronous child `Window` materialization (2).
4. `domparsing/` is scored at 28.4% (21/74). The rest is: the tentative
   streaming API (`Element.streamHTML`, `streamPositionalHTML`,
   `document.createParserOptions`; dozens of files, never started), a missing
   `Range.createContextualFragment` (35 subtests), `insertAdjacentHTML` gaps
   (`insert-adjacent`, `insert_adjacent_html`), XHTML `innerHTML`,
   `parsed-document-origin` (needs `Document.parseHTMLUnsafe`,
   `Document.parseHTML`, and `implementation.createDocument` arity), and
   `DOMParser-parseFromString-url-moretests` crossing-navigation cases
   (a realm's world is dropped on navigation; browsers keep the old realm
   usable).

Gotchas:

- The shared WPT checkout is the primary worktree's `third_party/wpt`;
  scratch tests go there and must be deleted.
- Killed WPT runs leave orphan `tinybrowser renderer` processes and a stale
  lock in `~/.cache/tinybrowser/`; kill them and rerun with
  `TINYBROWSER_WPT_NO_LOCK=1` when alone.
- `tests/wpt/metadata` is unchanged; scores are file-level, so partial
  subtest wins do not move a group until a whole file passes.

## Adversarial review pass (2026-09-19)

Five reviewers covered all web API/bindings areas (`69bcecf`, `1da7633`,
`ba1b4d7`). All 42 must-fix findings are closed except 6 downgraded after
verification against code+spec:

- Fixed: pristine-intrinsic conversions, trusted-bridge token gate,
  `__tb*` freeze, parser instance-realm URL, real cross-document adoption,
  mutation delivery/options, observer lifecycle, focus/click guards, six DOM
  core fixes, base URL, location hash, createDocument, budget nesting,
  window.event restore, cancelled timers, locale gate, IPv6 host, Intl exact
  decimals, input performer (5), UI event conversions (3).
- Downgraded (verified safe, no change): cross-realm `wrap_node` cache (cache
  is per-World, no global registry exists), `REALM_WORLDS` raw-pointer keys
  (insert overwrites stales, drop removes while owned), `frame_global`
  (same-origin gate present; same-origin direct access is per-spec),
  `document_url_string` fallback (returns `about:blank`, not the parent URL),
  `post_window_message` silent drops (host-internal wire, JS wrapper
  validates), `blur_node` null relatedTarget (correct for blur-to-nothing).
- Regressions caught by gates during the fix and repaired: realm-teardown
  GC abort (new `Persistent`s needed `release_host_primitives` in
  `JsRealm::drop`), frozen `__tb_fetchSeq` breaking `fetch` (freeze is
  function-valued only), cross-realm DOMParser URL check (instance realm
  wins, not calling realm).
- Verification: clippy clean, 34 cargo suites, Playwright 9 passed + 1
  skipped (new `intl.spec.ts` gate), WPT `domparsing` 21/74 (matches
  pre-fix baseline), MutationObserver subset matches baseline, CDP `--all`
  re-run pending.
- Should-fix backlog (untouched except where noted), grouped by area:
  DOM core: `importNode` missing-`deep` must throw `TypeError`
  (`node.rs:585`); `Attr.value`/`nodeValue` need `LegacyNullToEmptyString`
  (`attributes.rs:314`); `getRootNode` ignores `composed` (`node.rs:2357`);
  `elementsFromPoint` must list all elements at the point, not the ancestor
  chain (`node.rs:433`); `setAttributeNS` skips `after_attribute_change`
  (`node.rs:1974`); `className` setter bypasses attr refresh, staling `Attr`
  wrappers (`node.rs:1816`); `set_attr_value` writes the registry before the
  DOM write succeeds (`attributes.rs:620`); `contains`/`isSameNode` must throw
  `TypeError` for non-`Node` (`node.rs:2337/2316).
  Document/window/focus: removal focus fixup should move toward parent/body,
  not `None` (`focus.rs:18`); `blur_node` needs a focus-chain check
  (`focus.rs:274`). (Disabled-click guard, click-focuses-first, and
  `observe` presence/`null` were fixed with the must-fix batch.)
  Web APIs: legacy `keypress` order (`input.js`, old `:2845`); canceled
  `wheel` must not scroll (`input.js`, old `:2856`); contentEditable insert
  must respect the caret, not append to `textContent` (`input.js`, old
  `:2836`); `URLSearchParams` percent-decoding of split UTF-8 (`url.js`, old
  `:1107`); `URL` brand checks (`url.js`, old `:1014`); `pointerType: ""`
  coercion (`ui_events.js`, old `:2617`); `InputEvent.data` string conversion
  (`ui_events.js`, old `:2709`); swallowed focus error in the performer
  (`input.js`, old `:2788`).
  IDL/infra: `DOMException` name mapping for `null`/`""`
  (`exceptions.rs:57`); `unsigned long` precision above 2^53
  (`webidl.rs:122`); silent `Ok(())` on bad message wire data
  (`messaging.rs:84`); `HTMLCollection` `ownKeys`/`deleteProperty`/method
  identity/indexed-`set` traps (`collections.js:85/74/106`); cross-realm
  `CustomEvent.detail` symbol (`events/custom_event.js:2`); `signalAbort`
  trust + `timeout` clamp (`events/abort.js:17`); script geometry ignores
  stacking/`pointer-events` and external sheets (`bindings/mod.rs:357`).
  Engine glue: `Drop for JsRealm` can panic inside `Drop` (`js/mod.rs:775`);
  timer clamp should be >2^31-1 to 1ms (`js/mod.rs:960`); unaddressable u64
  ids become silent `NaN` (`js/mod.rs:970`); listener exceptions never reach
  `window.onerror` (`events.rs:1023`); refused opaque-origin storage writes
  report success (`world.rs:823`); `timeStyle`/`useGrouping` validation
  timing (`intl.js:576/312`); deep WebDriver JSON silently nulls past depth
  32 (`js/mod.rs:904`).
- Note: `tools/intl/test262` runner is stale (binary CLI lost
  `create`/`eval`/`close --profile`); Intl is verified via `intl.spec.ts`.
