# Handoff (2026-09-19)

State: branch `chase/wpt-grind` (not pushed), tip `4cfbca2`, clean. Scores on
this branch: `webstorage/` 47/54 (87.0%), `url/` 25/49 (51.0%),
`webmessaging/` 106/124 (85.5%, broadcastchannel excluded),
`webmessaging/broadcastchannel/` 5/12 (41.7%), `focus/` 3/41 (7.3%),
`domparsing/` 21/74 (28.4%), `FileAPI/` 32/68 (47.1%). Shipping binary
8,282,776 bytes (cap 10,485,760). CDP `--all`: PASS 33, no missing methods
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
