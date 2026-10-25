# Limitations

Honest current gaps. Upstream engine gaps live in docs/upstream.md;
out-of-scope API families live in docs/priorities.md.

## JS engine

QuickJS bytecode interpreter, no JIT. DOM glue and agent scripts are
fine; JS-heavy pages run slower than V8 browsers. No benchmark yet.

## Rendering

One-shot PNG only: CDP Page.captureScreenshot is PNG-only
(crates/cdp/src/dispatch.rs:882), no continuous paint loop, no GPU,
no vsync, no video. requestAnimationFrame is a 16ms timer with
documented deviations: shared timer table, Date.now() timestamps,
no per-frame timestamp sharing (timers.js:63).

## Intl

NumberFormat and DateTimeFormat only. No Collator, PluralRules,
RelativeTimeFormat, ListFormat, or Segmenter. Real CLDR data for 7
locales (en-US es-ES de-DE ja-JP fr-FR zh-CN ko-KR,
tools/intl/generate-data); every other locale silently falls back.

## Strings

Lone surrogates round-trip through character data, titles, and
attributes. The layout tree still stores UTF-8 (docs/upstream.md).

## CSS colors

Canvas styles and `<input type=color>` use the CSS Color 4 parser in
`crates/renderer/src/css/color.rs`. It implements the predefined `color()`
spaces, but clips out-of-gamut colors per channel instead of running OKLCH
local-MINDE gamut mapping. Wide-gamut colors far outside sRGB can differ from
a browser that implements the mapping algorithm.

## Missing surfaces

No workers or service-workers. Canvas `getContext` returns a working 2D
context (paths, text, gradients, patterns, filters, layers); the
`bitmaprenderer` context exists but transfers nothing until `ImageBitmap`
decoding lands; WebGL returns null. Sync XHR is rejected, customized
built-in elements are rejected. Shadow-scoped styles and composed-event
retargeting are upstream gaps (docs/upstream.md).

`Request` exposes `blob()` only of the body-consumption methods; `text`,
`json`, `arrayBuffer`, and `bytes` are missing. A `text/plain` document
is still parsed as HTML, so markup served as plain text renders instead
of showing as text (WPT `mimesniff/sniffing/`).

Cross-origin `fetch`/XHR run the CORS protocol in the browser process
(`crates/net/src/cors.rs`): the CORS check, preflight, credentials mode,
response-header filtering, opaque `no-cors`, redirect-tainted origins,
and redirect mode. A `no-cors` request is held to CORS-safelisted methods
and headers at the network boundary, but the JS-visible `Request.headers`
and `Headers.set` do not yet apply the `request-no-cors` guard, so a page
sees headers in its own object that the wire drops. Still open: the
CORS-preflight cache (every non-simple request preflights again),
`requestInit.referrerPolicy`/`referrer` on a request, `keepalive`, request
origins for sandboxed and `data:` frames (sent as the URL origin, not
`null`), cookies on a credentialed cross-origin redirect, and deriving the
request origin from a per-frame browser record rather than the
site-authorized initiator (a renderer may name any origin within its own
site, which shares its process). A header value holding a control byte
other than TAB (Fetch permits e.g. `0x01`) is refused by the HTTP layer,
so WPT `fetch/api/cors/cors-preflight-not-cors-safelisted.any.js` still
fails on the `accept-language`/`content-language` `0x01` rows. `fetch`
also ignores request `integrity` metadata
(SRI is enforced for scripts only, `crates/renderer/src/csp.rs`); the old
cross-origin block hid both that and the sandbox-origin gap, so
`fetch/api/basic/integrity.sub.any.html` and
`xhr/access-control-sandboxed-iframe-*.htm` now fail honestly. A
same-origin `cors` POST that survives a 307/308 redirect sends `Origin`,
which `fetch/api/redirect/redirect-origin.any.js` expects to be absent;
step 3 of the append algorithm does not exempt that case, and Chrome and
Firefox fail the same two subtests. WPT `fetch/api/cors/` covers the
surface.

CSP `eval`/`Function` gating shadows the globals in JS because the
engine has no compilation hook; direct `eval` therefore evaluates
indirectly (global scope) even when allowed. Blocking and report
delivery (`report-uri`/`report-to`), `frame-ancestors`, and per-element
violation source positions are not implemented yet.

Associated stylesheets expose `cssRules`, per-interface rule objects, and
declaration accessors. Still open: constructable `insertRule` /
`deleteRule` throw `NotSupportedError`, and a cross-origin `<link>` is
never origin-clean because the fetch path does not record a CORS
approval. Conformance for this surface is WPT `css/cssom/`.
