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

Slices, not full-group percentages: html5lib (`html/syntax/parsing/html5lib_*.html`, 173 as expected on 2026-09-15), `dom/events/`, `dom/nodes/` (275/354 on 2026-10-08), `css/selectors/`, `fetch/api/`, and the CSS reftest slice `css/css-color/` (266/307 on 2026-09-18 with `--test-types reftest`).

```sh
nix develop --command ./tools/wpt/run --score <directory> -- --exclude=worker
```

| directory | percentage |
| --- | --- |
| total | 14.1% |
| `accelerometer/` | 8.3% |
| `accessibility/` | 81.0% |
| `accname/` | 0.0% |
| `acid/` | 0.0% |
| `ai/` | 0.6% |
| `ambient-light/` | 10.0% |
| `animation-worklet/` | 0.0% |
| `annotation-model/` | unscored |
| `annotation-protocol/` | unscored |
| `annotation-vocab/` | unscored |
| `apng/` | 0.0% |
| `appmanifest/` | unscored |
| `audio-output/` | 0.0% |
| `audio-session/` | 0.0% |
| `autoplay-policy-detection/` | 33.3% |
| `avif/` | unscored |
| `background-fetch/` | 9.1% |
| `background-sync/` | 50.0% |
| `badging/` | 20.0% |
| `battery-status/` | 0.0% |
| `beacon/` | 0.0% |
| `bluetooth/` | 0.6% |
| `captured-mouse-events/` | 25.0% |
| `clear-site-data/` | 7.7% |
| `client-hints/` | 33.3% |
| `clipboard-apis/` | 5.0% |
| `close-watcher/` | 0.0% |
| `compat/` | 23.1% |
| `compression/` | 0.0% |
| `compute-pressure/` | 0.0% |
| `connection-allowlist/` | 8.1% |
| `console/` | 16.7% |
| `contacts/` | 0.0% |
| `container-timing/` | 0.0% |
| `content-dpr/` | 0.0% |
| `content-index/` | 0.0% |
| `content-security-policy/` | 11.5% |
| `contenteditable/` | 33.3% |
| `cookies/` | 29.5% |
| `cookiestore/` | 0.0% |
| `core-aam/` | 0.0% |
| `cors/` | 0.0% |
| `cpu-performance/` | 0.0% |
| `credential-management/` | 6.2% |
| `css/` | 17.2% |
| `css/selectors/` (slice) | 7.0% |
| `cssom/` | 100.0% |
| `custom-elements/` | 4.6% |
| `delegated-ink/` | 0.0% |
| `density-size-correction/` | 0.0% |
| `deprecation-reporting/` | 100.0% |
| `device-bound-session-credentials/` | 0.0% |
| `device-memory/` | 50.0% |
| `device-posture/` | 0.0% |
| `digital-credentials/` | 3.7% |
| `direct-sockets/` | 0.0% |
| `document-picture-in-picture/` | 4.5% |
| `document-policy/` | 0.0% |
| `dom/` | 30.2% |
| `dom/nodes/` (slice) | 77.7% |
| `domparsing/` | 27.0% |
| `domxpath/` | 5.9% |
| `dpub-aam/` | 0.0% |
| `dpub-aria/` | unscored |
| `ecmascript/` | 50.0% |
| `editing/` | 16.7% |
| `element-timing/` | 0.0% |
| `encoding/` | 77.3% |
| `encoding-detection/` | 0.0% |
| `encrypted-media/` | 1.0% |
| `entries-api/` | 0.0% |
| `event-timing/` | 1.4% |
| `eventsource/` | 0.0% |
| `eyedropper/` | 50.0% |
| `fedcm/` | 1.2% |
| `fenced-frame/` | 1.7% |
| `fetch/` | 5.7% |
| `fetch/api/` (slice) | 7.4% |
| `file-system-access/` | 20.0% |
| `FileAPI/` | 50.0% |
| `focus/` | 7.3% |
| `font-access/` | 0.0% |
| `forced-colors-mode/` | 14.3% |
| `fs/` | 3.0% |
| `fullscreen/` | 4.2% |
| `gamepad/` | 16.7% |
| `generic-sensor/` | 0.0% |
| `geolocation/` | 9.5% |
| `geolocation-sensor/` | 100.0% |
| `gif/` | unscored |
| `gpc/` | 25.0% |
| `graphics-aam/` | unscored |
| `graphics-aria/` | 0.0% |
| `gyroscope/` | 10.0% |
| `hr-time/` | 7.7% |
| `hsts/` | 100.0% |
| `html/` | 13.9% |
| `html-aam/` | 0.0% |
| `html-longdesc/` | unscored |
| `html-media-capture/` | 50.0% |
| `html-ruby-extensions/` | unscored |
| `https-upgrades/` | 16.7% |
| `idle-detection/` | 0.0% |
| `imagebitmap-renderingcontext/` | 0.0% |
| `import-maps/` | 1.8% |
| `IndexedDB/` | 0.4% |
| `inert/` | 3.4% |
| `infrastructure/` | 23.1% |
| `input-device-capabilities/` | 0.0% |
| `input-events/` | 0.0% |
| `installedapp/` | 50.0% |
| `intersection-observer/` | 9.3% |
| `intervention-reporting/` | 100.0% |
| `is-input-pending/` | 0.0% |
| `jpegxl/` | 0.0% |
| `js/` | 57.7% |
| `js-self-profiling/` | 0.0% |
| `keyboard-lock/` | 0.0% |
| `keyboard-map/` | 0.0% |
| `largest-contentful-paint/` | 0.0% |
| `layout-instability/` | 0.0% |
| `loading/` | 5.7% |
| `long-animation-frame/` | 0.0% |
| `longtask-timing/` | 0.0% |
| `magnetometer/` | 10.0% |
| `managed/` | 0.0% |
| `mathml/` | 37.6% |
| `measure-memory/` | 7.1% |
| `media-capabilities/` | 0.0% |
| `media-playback-quality/` | 0.0% |
| `media-source/` | 1.2% |
| `mediacapture-extensions/` | 0.0% |
| `mediacapture-fromelement/` | 0.0% |
| `mediacapture-handle/` | 0.0% |
| `mediacapture-image/` | 4.2% |
| `mediacapture-insertable-streams/` | 0.0% |
| `mediacapture-record/` | 0.0% |
| `mediacapture-region/` | 0.0% |
| `mediacapture-streams/` | 3.4% |
| `mediasession/` | 0.0% |
| `merchant-validation/` | 0.0% |
| `mimesniff/` | 0.0% |
| `mixed-content/` | 0.0% |
| `mst-content-hint/` | 0.0% |
| `nav-tracking-mitigations/` | 0.0% |
| `navigation-api/` | 0.4% |
| `navigation-timing/` | 3.4% |
| `netinfo/` | 0.0% |
| `network-error-logging/` | 0.0% |
| `notifications/` | 0.0% |
| `old-tests/` | 6.7% |
| `orientation-event/` | 0.0% |
| `orientation-sensor/` | 6.2% |
| `page-lifecycle/` | 20.0% |
| `page-visibility/` | 0.0% |
| `paint-timing/` | 0.0% |
| `payment-method-basic-card/` | 0.0% |
| `payment-method-id/` | 0.0% |
| `payment-method-manifest/` | 0.0% |
| `payment-request/` | 0.0% |
| `performance-timeline/` | 2.0% |
| `periodic-background-sync/` | 0.0% |
| `permissions/` | 7.1% |
| `permissions-policy/` | 0.0% |
| `permissions-request/` | 0.0% |
| `permissions-revoke/` | 0.0% |
| `picture-in-picture/` | 0.0% |
| `png/` | 0.0% |
| `pointerevents/` | 5.8% |
| `pointerlock/` | 5.0% |
| `preload/` | 3.5% |
| `presentation-api/` | 0.0% |
| `print/` | 100.0% |
| `private-click-measurement/` | 100.0% |
| `proximity/` | 25.0% |
| `push-api/` | 0.0% |
| `quirks/` | 15.0% |
| `referrer-policy/` | 3.3% |
| `remote-playback/` | 0.0% |
| `reporting/` | 4.0% |
| `requestidlecallback/` | 0.0% |
| `resize-observer/` | 0.0% |
| `resource-timing/` | 0.0% |
| `sanitizer-api/` | 0.0% |
| `savedata/` | 0.0% |
| `scheduler/` | 0.0% |
| `screen-capture/` | 6.7% |
| `screen-details/` | 0.0% |
| `screen-orientation/` | 0.0% |
| `screen-wake-lock/` | 6.2% |
| `scroll-animations/` | 5.9% |
| `scroll-performance-timing/` | 0.0% |
| `scroll-to-text-fragment/` | 0.0% |
| `secure-contexts/` | 0.0% |
| `secure-payment-confirmation/` | 0.0% |
| `selection/` | 25.0% |
| `serial/` | 0.0% |
| `server-timing/` | 11.1% |
| `service-workers/` | 0.7% |
| `shadow-dom/` | 21.9% |
| `shape-detection/` | 0.0% |
| `signed-exchange/` | 5.0% |
| `soft-navigation-heuristics/` | 0.0% |
| `speculation-rules/` | 0.4% |
| `speech-api/` | 0.0% |
| `storage/` | 0.0% |
| `storage-access-api/` | 2.5% |
| `streams/` | 5.3% |
| `subapps/` | 0.0% |
| `subresource-integrity/` | 3.3% |
| `svg/` | 6.6% |
| `svg-aam/` | 0.0% |
| `third_party/` | unscored |
| `timing-entrytypes-registry/` | 0.0% |
| `top-level-storage-access-api/` | 0.0% |
| `touch-events/` | 13.3% |
| `trust-tokens/` | 0.0% |
| `trusted-types/` | 3.4% |
| `ua-client-hints/` | 50.0% |
| `uievents/` | 19.4% |
| `upgrade-insecure-requests/` | 0.0% |
| `url/` | 51.0% |
| `urlpattern/` | 0.0% |
| `user-timing/` | 2.8% |
| `vibration/` | 25.0% |
| `video-rvfc/` | 12.5% |
| `viewport/` | 0.0% |
| `viewport-segments/` | 0.0% |
| `virtual-keyboard/` | 0.0% |
| `visual-viewport/` | 0.0% |
| `wai-aria/` | 3.1% |
| `wasm/` | 0.0% |
| `web-animations/` | 13.7% |
| `web-based-payment-handler/` | 33.3% |
| `web-bundle/` | 0.0% |
| `web-extensions/` | 0.0% |
| `web-install/` | 0.0% |
| `web-locks/` | 2.2% |
| `web-nfc/` | 0.0% |
| `web-otp/` | 100.0% |
| `web-share/` | 20.0% |
| `webaudio/` | 1.7% |
| `webauthn/` | 3.8% |
| `webcodecs/` | 2.8% |
| `WebCryptoAPI/` | 1.9% |
| `webdriver/` | 0.0% |
| `webgl/` | 10.0% |
| `webhid/` | 20.0% |
| `webidl/` | 17.8% |
| `webmcp/` | 6.7% |
| `webmessaging/` | 76.4% |
| `webmessaging/broadcastchannel/` | 41.7% |
| `webmidi/` | 0.0% |
| `webnn/` | 0.2% |
| `webrtc/` | 0.4% |
| `webrtc-encoded-transform/` | 0.0% |
| `webrtc-extensions/` | 0.0% |
| `webrtc-ice/` | 0.0% |
| `webrtc-identity/` | 0.0% |
| `webrtc-priority/` | 0.0% |
| `webrtc-stats/` | 11.1% |
| `webrtc-svc/` | 0.0% |
| `websockets/` | 0.0% |
| `webstorage/` | 90.7% |
| `webtransport/` | 0.0% |
| `webusb/` | 0.0% |
| `webvtt/` | 0.0% |
| `webxr/` | 4.8% |
| `window-management/` | 0.0% |
| `workers/` | 2.4% |
| `worklets/` | 0.0% |
| `x-frame-options/` | 33.3% |
| `xhr/` | 2.9% |
| `xml/` | 47.1% |

## Upstream Blitz bugs (known-fail, not worked around)

Policy: upstream Blitz bugs stay upstream. Our code stays simple; these fail closed as known-fails.

- `blitz-dom-0.3.0-beta.2/src/document.rs:1086` `resolve_url` panics on unresolvable relative refs (observed: `foo.jpg` against `data:text/css` base). Upstream `main` still panics the same way.
- `blitz-dom` has no PI / CDATA / doctype node kinds; parsing drops the doctype. Our surface stays simple: `createProcessingInstruction` / `createCDATASection` throw, `document.doctype` reads null, doctype arguments are dropped. These fail as known-fails.
- `blitz-dom` has no shadow DOM: no shadow roots, `attachShadow` never hosts, `ShadowRoot` brand never instantiates, `getComposedRanges` / composed options are no-ops.
- `blitz-dom` has no template contents: `<template>` children live as ordinary element children.
- `blitz-dom` keeps no form-control state: no dirty value flag, checkedness, selectedness, or indeterminate slots; values read from content attributes and descendant text, `select` events have no producer, form-state cloning carries structure only.
- `blitz-dom` exposes no image-request state: `image_cache` / `pending_images` are `pub(crate)`, so request tracking (selected `currentSrc`, loading/broken flags, `load` / `error` events) stays ours; the decoded result itself is public (`ElementData::image_data`), so `naturalWidth` / `naturalHeight` and the `load` / `error` decision read Blitz state with no second decode. SVG and GIF `<img>` now load like Chromium.
- `blitz-dom` exposes no public visibility helper: one `style::` use remains for the `visibility` check.
- `blitz-dom` styles with hardcoded `NoQuirks` internally, so `document.compatMode` (sniffed from the doctype) can disagree with the rendering mode.
- `blitz-dom` puts `AnonymousBlock` layout boxes in the same tree; we treat them as transparent (snapshot/serialize children only, never brand as elements).
- `blitz-dom` exposes only the viewport scroll offset, no per-element scroll-container offset API.
- `blitz-dom` owns document language internally with no metadata setter, so a response `Content-Language` is not applied.
- `blitz-html` always parses scripting-disabled (`noscript` as markup); scripting-enabled `noscript`-as-text diverges.
- `blitz-dom-0.3.0-beta.2/src/stylo.rs:433` `NonTSPseudoClass::PlaceholderShown` is hardcoded `false`, so `:placeholder-shown` never matches and placeholder text does not paint (observed: Wikipedia's "Search Wikipedia").
- Pre-existing `main` crashes, out of scope: `dom/nodes/Document-characterSet-normalization-1.html`, `Document-characterSet-normalization-2.html`, `Document-createElement-namespace.html`.
