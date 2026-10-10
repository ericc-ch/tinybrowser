# Progress

This file records the latest snapshot. After you score WPT or measure the binary, replace this snapshot.
Size method and marginals live in [`docs/size.md`](size.md).

## Binary size

Policy: minimize stripped size on x86_64; no hard cap.

10,726,824 bytes (2026-10-09)

```sh
nix develop --command ./tools/release
```

## WPT

Overnight dump 2026-09-27 14:56 WIB tip `6ec8696c8df2` (testharness+crashtest, `--exclude=worker`, processes=4 after mid-run restart (was 2 at start)). pass=5064 expfail=33 unexp=21463 timeout=3661 crash=121 error=5661 skip=78 tests=36081. Known-skips stay known-skips; not claimed green.

WPT has many test groups (html, css, dom, and more). Each percentage is the pass count over scored tests in that directory (testharness+crashtest, `--exclude=worker`). A directory stays unscored when the overnight runner got no usable report (empty/missing group or killed mid-restart). Unscored does not mean the tests are missing from WPT.

The **total** row is pass / (tests − skip) across every directory that produced a report in this overnight dump — not a claim that every WPT file on earth was run. Worker excludes and known-skips stay out of the fail pile.

Slices, not full-group percentages: html5lib (`html/syntax/parsing/html5lib_*.html`, 173 as expected on 2026-09-15), `dom/events/`, `dom/nodes/` (287/354 on 2026-10-09: 272 clean-run, 15 `moveBefore` timing flakes pass on rerun; 59 unexpected + 8 error remain), `css/selectors/`, `fetch/api/`, and the CSS reftest slice `css/css-color/` (266/307 on 2026-09-18 with `--test-types reftest`).

```sh
nix develop --command ./tools/wpt/run --score <directory> -- --exclude=worker
```

Total: 14.1% (pass / (tests - skip) from the overnight dump above).

Per-directory breakdown is not hand-maintained. Track the report result instead: keep the overnight `--save-report` JSON (zstd-compressed, see `tools/wpt/README.md`) and view it on demand with:

```sh
nix develop --command ./tools/wpt/run --score --report <report.json>
```

## Priorities

Context: headless for AI agents, not humans. Maximize conformance per byte; trade pixel-perfect rendering, tabs/themes/extensions/sync, and real-time media. Agent loop is navigate → read DOM → click/fill → extract (`https://blog.cloudflare.com/browser-run-for-ai-agents`, `https://agent-browser.dev`, `https://lightpanda.io/blog/posts/web-automation-stack-explained`). Real-world usage is HTTP Archive 2025 over 17.2M sites (`https://almanac.httparchive.org/en/2025/capabilities`, `https://almanac.httparchive.org/en/2025/webassembly`). Human-browser consensus is Interop 2026 (`https://web.dev/blog/interop-2026`, `https://webkit.org/blog/17818/announcing-interop-2026`); we follow it for document/actuation work and intentionally diverge on human-visual and real-time media. Closest comp is Kitesurf: agent-first, stateless, CDP-subset browser that got DOM 97% / HTML 96% / Selection 99% / Encoding 99% / XHR 95% / CORS 95% / URL 83% first, skips video/WebGL, added WebMCP (`https://developers.cloudflare.com/browser-run/kitesurf`, `https://blog.cloudflare.com/kitesurf`, `https://blog.cloudflare.com/kitesurf-update`).

- P0 document core (every page breaks without it; also Kitesurf's 95%+ band): `html/`, `dom/`, `domparsing/`, `encoding/`, `url/`, `fetch/`, `xhr/`, `css/` + `css/selectors/` + `cssom/`, `js/` + `ecmascript/`, `webidl/`, `infrastructure/`, `mimesniff/`, `cookies/`, `webstorage/`, `content-security-policy/`, `referrer-policy/`, `webmessaging/`.
- P1 agent actuation (modern app flows: SPA navigation, components, editing, background work): `navigation-api/` (Interop 2026), `custom-elements/` + `shadow-dom/` + scoped registries (Interop 2026), `editing/` + `contenteditable/` + `selection/` + `clipboard-apis/` (Clipboard is ~11-12% usage, ex-`execCommand` replacement), `uievents/` + `focus/` + minimal `pointerevents/`, `workers/` + `service-workers/`, `IndexedDB/` (Interop 2026 `getAllRecords()`), `streams/` + `compression/` (`CompressionStream` is the #1 capability at 12-14%, `https://almanac.httparchive.org/en/2025/capabilities`), `FileAPI/`, `trusted-types/`, `web-locks/`, `import-maps/` (Kitesurf update added CSSOM/Typed OM/custom-elements/URL+JSON-module/import-map handling, `https://blog.cloudflare.com/kitesurf-update`).
- P2 vetoed in, kept: `wasm/` (0.35% desktop / 0.28% mobile overall but 2% of top-1000 complex apps and 5.5% of Chrome-visited pages, `https://almanac.httparchive.org/en/2025/webassembly`, `https://platform.uno/blog/the-state-of-webassembly-2025-2026`; JSPI is Interop 2026) + `webmcp/` (agent tools beat click-loops: `document.modelContext.registerTool`, Permissions-Policy `tools`, origin trial from Chrome 149, `https://developer.chrome.com/docs/ai/webmcp`, spec `https://webmachinelearning.github.io/webmcp`; Kitesurf + Browser Run ship it, `https://blog.cloudflare.com/kitesurf-update`, `https://blog.cloudflare.com/browser-run-for-ai-agents`) + `ai/` (built-in AI <1% today, first measurable baseline, `https://almanac.httparchive.org/en/2025/capabilities`).
- Skip rule: needs hardware we don't have, human body/window presence, real-time media/GPU pipeline, or OS/chrome UI — high bytes, ~zero agent-task value. Fail closed, no stubs.
  - Hardware: `bluetooth/`, `webusb/`, `serial/`, `webhid/`, `web-nfc/`, `webmidi/`, `gamepad/`, `accelerometer/` + `gyroscope/` + `magnetometer/` + `generic-sensor/` + `ambient-light/` + `proximity/` + `orientation-sensor/`, `battery-status/`, `geolocation/` (live position; fixed egress only) + `geolocation-sensor/`, `contacts/`, `font-access/`, `screen-details/`, `device-posture/` + `viewport-segments/` + `window-management/`, `eyedropper/`, `shape-detection/`.
  - Human window/modal: `vibration/`, `screen-wake-lock/` + `idle-detection/`, `fullscreen/`, `pointerlock/`, `document-picture-in-picture/` + `picture-in-picture/`, `notifications/` + `push-api/`, `presentation-api/` + `remote-playback/`, `keyboard-lock/` + `keyboard-map/` + `virtual-keyboard/`, `payment-request/` + `payment-method-basic-card/` + `payment-method-id/` + `payment-method-manifest/` + `secure-payment-confirmation/` + `merchant-validation/` + `web-based-payment-handler/` (agent checkout goes via forms/WebMCP tools, not the payment sheet), `credential-management/` + `webauthn/` + `fedcm/` + `digital-credentials/` (live ceremonies are the Human-in-Loop wall; DOM login forms stay P1).
  - Real-time media/GPU (DOM surface only, no decode/playback/pipeline): `webaudio/` + `audio-output/` + `audio-session/`, `media-source/` + `media-playback-quality/` + `mediasession/` + `autoplay-policy-detection/`, `mediacapture-extensions/` + `mediacapture-fromelement/` + `mediacapture-handle/` + `mediacapture-image/` + `mediacapture-insertable-streams/` + `mediacapture-record/` + `mediacapture-region/` + `mediacapture-streams/` + `html-media-capture/`, `webcodecs/`, `webgl/`, `webxr/`, `speech-api/`, `screen-capture/`. Media elements keep element/attribute surface + `media-capabilities/decodingInfo()` compat only. Intentionally diverges from Interop 2026 on `webrtc*/` (`webrtc/`, `webrtc-encoded-transform/`, `webrtc-extensions/`, `webrtc-ice/`, `webrtc-identity/`, `webrtc-priority/`, `webrtc-stats/`, `webrtc-svc/`, `mst-content-hint/`): human-browser priority, prohibitive bytes, agents never place calls.
  - Deprioritized behind selectors/CSSOM, not skipped: visual-fidelity CSS (`scroll-animations/`, `view-transitions/`, anchor positioning, `contrast-color()`, `shape()`, `zoom/`, `scroll-snap/`).

## Upstream Blitz bugs (known-fail, not worked around)

Policy: upstream Blitz bugs stay upstream. Our code stays simple; these fail closed as known-fails.
Fixed in our `ericc-ch/blitz` fork (`third_party/blitz`, `master` is the line)
so far: `resolve_url` no longer panics (returns `None`, callers skip);
real doctype / processing-instruction / CDATA section node kinds with
constructors, sink preservation (except the XML declaration, which is
prolog), serialization, text, and layout treatment; real template
contents fragments with host links, parser/fragment routing, cloning,
and cycle visibility; sink parse errors carried on the document for
draining; internal general entities expanded up front with
billion-laughs bounds; `Node::is_visible` for computed visibility.
Fixed in our `ericc-ch/html5ever` fork (`third_party/html5ever`,
`master` is the line, from the byte-identical 0.39.0 base) so far:
namespace declarations kept on elements; CDATA sections tokenized and
built as real nodes; EOF-with-open-elements and PUBLIC-without-SYSTEM
reported; internal DTD subsets skipped instead of going bogus.

- `blitz-dom` stores text as UTF-8, so lone surrogates in `CharacterData`, attributes, and titles read back as U+FFFD. No side table preserves them; fails as known-fails.
- `blitz-dom` has no shadow DOM: no shadow roots, `attachShadow` never hosts, `ShadowRoot` brand never instantiates, `getComposedRanges` / composed options are no-ops.
- Template contents share their host document instead of a separate inert template-contents owner document, so `content.ownerDocument !== document` assertions fail. Needs per-document inert owner documents with cross-arena contents routing; deferred as its own feature.
- XML namespace prefix synthesis on serialization is skipped (stored qualified names used as-is); entity expansion covers internal general entities only.
- `blitz-dom` keeps no form-control state: no dirty value flag, checkedness, selectedness, or indeterminate slots; values read from content attributes and descendant text, `select` events have no producer, form-state cloning carries structure only.
- `blitz-dom` exposes no image-request state: `image_cache` / `pending_images` are `pub(crate)`, so request tracking (selected `currentSrc`, loading/broken flags, `load` / `error` events) stays ours; the decoded result itself is public (`ElementData::image_data`), so `naturalWidth` / `naturalHeight` and the `load` / `error` decision read Blitz state with no second decode. SVG and GIF `<img>` now load like Chromium.
- `blitz-dom` styles with hardcoded `NoQuirks` internally, so `document.compatMode` (sniffed from the doctype) can disagree with the rendering mode.
- `blitz-dom` puts `AnonymousBlock` layout boxes in the same tree; we treat them as transparent (snapshot/serialize children only, never brand as elements).
- `blitz-dom` exposes only the viewport scroll offset, no per-element scroll-container offset API.
- `blitz-dom` owns document language internally with no metadata setter, so a response `Content-Language` is not applied.
- `blitz-html` always parses scripting-disabled (`noscript` as markup); scripting-enabled `noscript`-as-text diverges.
- `blitz-dom` `NonTSPseudoClass::PlaceholderShown` is hardcoded `false`, so `:placeholder-shown` never matches and placeholder text does not paint (observed: Wikipedia's "Search Wikipedia").
- Proposal tests fail everywhere by design: HTML `<?...?>` stays a bogus comment per the HTML Standard (the PI-attributes proposal expects PI nodes), and two PI value subtests (`axx>`, `some<>`) contradict the spec's own escaping algorithm.
