# Credits

Simple acknowledgements for code, data, tests, and prior art used by tinybrowser.

License texts: [LICENSE](LICENSE) (MIT for our code) and [NOTICE](NOTICE) (third-party sticky terms).

## Engines and browsers

- [Servo](https://servo.org/) ([Stylo](https://github.com/servo/stylo), [html5ever](https://crates.io/crates/html5ever), [selectors](https://crates.io/crates/selectors), [cssparser](https://crates.io/crates/cssparser), and related crates)
- [Chromium](https://www.chromium.org/) / [Blink](https://www.chromium.org/blink/) (behavioral reference, CDP fixtures, several algorithms)
- [Firefox](https://www.mozilla.org/firefox/) / [Gecko](https://firefox-source-docs.mozilla.org/) / [SpiderMonkey](https://spidermonkey.dev/) (behavioral reference, several algorithms, Intl boundary)
- [WebKit](https://webkit.org/) (docs-only third read when Chromium and Firefox disagree)
- [QuickJS](https://bellard.org/quickjs/) (Fabrice Bellard) → [QuickJS-NG](https://github.com/quickjs-ng/quickjs) → [rquickjs](https://github.com/DelSkayn/rquickjs) (DelSkayn) → [project forks](https://github.com/ericc-ch/rquickjs)

## Inspiration and prior art

- [Obscura](https://github.com/h4ckf0r0day/obscura) (CPU paint over Taffy)
- [Kitesurf](https://developers.cloudflare.com/browser-run/kitesurf/) (agent-first trade-offs)
- [Blitz](https://github.com/DioxusLabs/blitz) / [DioxusLabs](https://github.com/DioxusLabs) (parse/style/layout/paint owner: `blitz-dom`/`blitz-html`/`blitz-paint`/`blitz-traits` 0.3.0-beta.2 + `anyrender` 0.13.0)
- [NetSurf](https://www.netsurf-browser.org/) (small-engine pipeline prior art)
- [Dillo](https://dillo-browser.github.io/) (style → layout → canvas prior art)
- [Effect Logger](https://github.com/Effect-TS/effect) (logging crate model)

## Direct Rust dependencies (by role)

Versions move; check `Cargo.lock`. Notable licenses called out.

### JS

- [rquickjs](https://crates.io/crates/rquickjs) (+ [core](https://crates.io/crates/rquickjs-core)/[macro](https://crates.io/crates/rquickjs-macro)/[sys](https://crates.io/crates/rquickjs-sys)), [patched fork](https://github.com/ericc-ch/rquickjs) wrapping QuickJS-NG

### HTML / DOM / CSS

- [html5ever](https://crates.io/crates/html5ever), [markup5ever](https://crates.io/crates/markup5ever)
- [stylo](https://crates.io/crates/stylo), [stylo_traits](https://crates.io/crates/stylo_traits) ([MPL-2.0](https://www.mozilla.org/MPL/2.0/))
- [url](https://crates.io/crates/url)

### Layout / text / paint

- [parley](https://crates.io/crates/parley) (system fonts via fontconfig)
- [png](https://crates.io/crates/png)
- [encoding_rs](https://crates.io/crates/encoding_rs) (((Apache-2.0 OR MIT) AND BSD-3-Clause))
- [flate2](https://crates.io/crates/flate2), [getrandom](https://crates.io/crates/getrandom)

Paint backend (`crates/anyrender-tiny-skia`): `tiny-skia`, `kurbo`, `skrifa`, `peniko` via `anyrender`, blur via `image`.

### Intl

- [icu_*](https://icu4x.unicode.org/) family and related crates ([Unicode-3.0](https://www.unicode.org/license.txt))
- locale data blob from [icu4x-datagen](https://crates.io/crates/icu4x-datagen)

### Net / runtime

- [hyper](https://crates.io/crates/hyper), [hyper-util](https://crates.io/crates/hyper-util), [http](https://crates.io/crates/http), [http-body-util](https://crates.io/crates/http-body-util), [hyper-tls](https://crates.io/crates/hyper-tls)
- [native-tls](https://crates.io/crates/native-tls), [tokio-native-tls](https://crates.io/crates/tokio-native-tls)
- [tungstenite](https://crates.io/crates/tungstenite), [tokio-tungstenite](https://crates.io/crates/tokio-tungstenite)
- [tokio](https://crates.io/crates/tokio), [futures-util](https://crates.io/crates/futures-util), [tower-service](https://crates.io/crates/tower-service), [bytes](https://crates.io/crates/bytes)
- [serde](https://crates.io/crates/serde), [serde_json](https://crates.io/crates/serde_json), [thiserror](https://crates.io/crates/thiserror), [base64](https://crates.io/crates/base64), [sha1](https://crates.io/crates/sha1)
- [wit-bindgen](https://crates.io/crates/wit-bindgen)

## Vendored trees and fixtures

- `third_party/wpt` — [web-platform-tests](https://github.com/web-platform-tests/wpt) (submodule)
- `third_party/blink-cdp/` — Chromium Blink [inspector-protocol](https://source.chromium.org/chromium/chromium/src/+/main:third_party/blink/web_tests/inspector-protocol/) web tests
- [test262](https://github.com/tc39/test262) (via WPT) for Intl tooling

## Fonts and data

- [Public Suffix List](https://publicsuffix.org/list/) (`crates/cookies/src/public_suffix_list.dat`) from [publicsuffix.org](https://publicsuffix.org/)

## Ports and algorithm references

Not a full copy of those engines; behavior taken from comments in-tree:

- Blink / Firefox MessagePort disentangle → `renderer/messaging.rs`
- Blink `DispatchMessageEventWithOriginCheck` → `renderer/engine.rs`
- Blink `[CrossOrigin]` + Firefox `sCrossOriginProperties` → `js/scripts/web/messaging.js`
- Chromium timer clamping ([crbug.com/1108877](https://crbug.com/1108877)) → `js/scripts/web/timers.js`
- Blink HTMLOptionsCollection 100k cap → `js/scripts/collections.js`
- Firefox [`nsDOMAttributeMap::GetSupportedNames`](https://searchfox.org/firefox-main/source/dom/base/nsDOMAttributeMap.cpp) → `js/bindings/attributes.rs`
- SpiderMonkey / Firefox Intl option boundary → `js/intl.rs`, `js/scripts/intl.js` (e.g. [NumberFormat.cpp](https://searchfox.org/firefox-main/source/js/src/builtin/intl/NumberFormat.cpp))
- Blink / Gecko realm-agnostic document store → `renderer/documents.rs`
- Chromium [process lock / site isolation](https://chromium.googlesource.com/chromium/src/+/main/docs/process_model_and_site_isolation.md#process-locks) → `browser/site.rs`
- Chrome navigation headers (Accept, Sec-Fetch-*, UA-CH) → `browser/network.rs`
- [html5lib](https://github.com/html5lib/html5lib-tests) `svg ` / `math ` foreign-content prefixes → renderer
- [CDP](https://chromedevtools.github.io/devtools-protocol/) and [WebDriver](https://www.w3.org/TR/webdriver2/) crates reimplement those protocols (not copies of Chrome)

Most other DOM / Fetch / HTML follows [WHATWG](https://spec.whatwg.org/) with engines as a behavioral check (Chromium first, then Firefox).

## Specs and protocols

- [WHATWG](https://spec.whatwg.org/) (DOM, HTML, Fetch, URL, Infra, Web IDL, messaging, …)
- [W3C WebDriver](https://www.w3.org/TR/webdriver2/)
- [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/)
- [ECMA-262](https://tc39.es/ecma262/) / [ECMA-402](https://tc39.es/ecma402/)
- Selected WebAppSec / [Fetch Metadata](https://w3c.github.io/webappsec-fetch-metadata/) / [UA-CH](https://wicg.github.io/ua-client-hints/) / [Upgrade-Insecure-Requests](https://www.w3.org/TR/upgrade-insecure-requests/)

## License notes

- [MPL-2.0](https://www.mozilla.org/MPL/2.0/): Stylo, selectors, cssparser, app_units (and related). Source offer / notices should stay in sync with those crates.
- [BSD-3-Clause](https://opensource.org/licenses/BSD-3-Clause): tiny-skia; parts of encoding_rs
- [Unicode-3.0](https://www.unicode.org/license.txt): ICU4X
- Public Suffix List: see upstream license at [publicsuffix.org](https://publicsuffix.org/)
