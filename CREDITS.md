# Credits

Simple acknowledgements for code, data, tests, and prior art used by tinybrowser.

## Engines and browsers

- Servo (Stylo, html5ever, selectors, cssparser, and related crates)
- Chromium / Blink (behavioral reference, CDP fixtures, several algorithms)
- Firefox / Gecko / SpiderMonkey (behavioral reference, several algorithms, Intl boundary)
- WebKit (docs-only third read when Chromium and Firefox disagree)
- QuickJS (Fabrice Bellard) → QuickJS-NG → rquickjs (DelSkayn) → project forks

## Inspiration and prior art

- Obscura (CPU paint over Taffy)
- Kitesurf (agent-first trade-offs)
- Blitz / DioxusLabs (size/quality benchmark; Taffy + Parley + Stylo integration pattern)
- NetSurf (small-engine pipeline prior art)
- Dillo (style → layout → canvas prior art)
- Effect Logger (logging crate model)

## Direct Rust dependencies (by role)

Versions move; check `Cargo.lock`. Notable licenses called out.

### JS

- rquickjs (+ core/macro/sys), patched fork wrapping QuickJS-NG

### HTML / DOM / CSS

- html5ever, markup5ever, tendril, web_atoms
- cssparser, selectors, precomputed-hash (MPL-2.0 where applicable)
- stylo, stylo_dom, stylo_static_prefs, stylo_traits, app_units (MPL-2.0)
- euclid, url

### Layout / text / paint

- taffy
- parley, fontique
- skrifa
- tiny-skia (BSD-3-Clause)
- kurbo
- png, zune-jpeg, zune-core, image-webp
- encoding_rs ((Apache-2.0 OR MIT) AND BSD-3-Clause)
- flate2, getrandom

### Intl

- icu_* family and related crates (Unicode-3.0)
- locale data blob from icu4x-datagen

### Net / runtime

- hyper, hyper-util, http, http-body-util, hyper-tls
- native-tls, tokio-native-tls
- tungstenite, tokio-tungstenite
- tokio, futures-util, tower-service, bytes
- serde, serde_json, thiserror, base64, sha1
- wit-bindgen

## Vendored trees and fixtures

- `third_party/wpt` — web-platform-tests (submodule)
- `third_party/blink-cdp/` — Chromium Blink inspector-protocol web tests
- test262 (via WPT) for Intl tooling

## Fonts and data

- Liberation Sans (OFL-1.1) under `crates/renderer/assets/`
- Public Suffix List (`crates/cookies/src/public_suffix_list.dat`) from publicsuffix.org

## Ports and algorithm references

Not a full copy of those engines; behavior taken from comments in-tree:

- Gecko `EnsureAllowedAsChild` → `dom/mutation/insert.rs`
- Blink / Firefox MessagePort disentangle → `renderer/messaging.rs`
- Blink `DispatchMessageEventWithOriginCheck` → `renderer/engine.rs`
- Blink `[CrossOrigin]` + Firefox `sCrossOriginProperties` → `js/scripts/web/messaging.js`
- Chromium timer clamping (crbug.com/1108877) → `js/scripts/web/timers.js`
- Blink HTMLOptionsCollection 100k cap → `js/scripts/collections.js`
- Chromium `DeselectItemsWithoutValidation` → `dom/form/select.rs`
- Firefox `nsDOMAttributeMap::GetSupportedNames` → `js/bindings/attributes.rs`
- SpiderMonkey / Firefox Intl option boundary → `js/intl.rs`, `js/scripts/intl.js`
- Chromium `html.css` form-submit UA sheet → `render/stylo.rs`
- Blitz Stylo prefs / container-query stub → stylo integration
- Blitz Taffy + own-paint split → `render/boxes.rs`
- Blink / Gecko realm-agnostic document store → `documents.rs`
- Chromium process lock / site isolation → `browser/site.rs`
- Chrome navigation headers (Accept, Sec-Fetch-*, UA-CH) → `browser/network.rs`
- html5lib `svg ` / `math ` foreign-content prefixes → renderer
- CDP and WebDriver crates reimplement those protocols (not copies of Chrome)

Most other DOM / Fetch / HTML follows WHATWG with engines as a behavioral check (Chromium first, then Firefox).

## Specs and protocols

- WHATWG (DOM, HTML, Fetch, URL, Infra, Web IDL, messaging, …)
- W3C WebDriver
- Chrome DevTools Protocol
- ECMA-262 / ECMA-402
- Selected WebAppSec / Fetch Metadata / UA-CH / Upgrade-Insecure-Requests

## License notes

- MPL-2.0: Stylo, selectors, cssparser, app_units (and related). Source offer / notices should stay in sync with those crates.
- BSD-3-Clause: tiny-skia; parts of encoding_rs
- Unicode-3.0: ICU4X
- OFL-1.1: Liberation Sans
- Public Suffix List: see upstream license at publicsuffix.org

