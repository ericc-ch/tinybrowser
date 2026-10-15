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

## Missing surfaces

No workers or service-workers. Canvas `getContext` returns a 2d or
bitmaprenderer object; WebGL returns null. Sync XHR is rejected,
customized built-in elements are rejected. Shadow-scoped styles are
an upstream gap (docs/upstream.md).
