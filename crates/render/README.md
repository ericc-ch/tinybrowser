# render

One-shot HTML/CSS rendering for screenshots: style, layout, paint, PNG.

This crate exists because the measured Blitz stack costs +6.6 MB against the
shipping binary (`docs/researches/size-budget.md`), which does not fit the
10 MB ceiling. The alternative shape is the one small browsers have always
used: a CSS subset over the DOM we already have, a box tree, and a CPU
rasterizer.

## Pipeline

1. **Style** (`style.rs`, `cascade.rs`) — parse `<style>` text and fetched
   sheets with `cssparser`, match selectors through the DOM's `selectors`
   integration, cascade to a computed `Style` per element. Follows CSS
   Cascade, CSS Values, and CSS Color.
2. **Box tree** (`tree.rs`) — anonymous block and inline boxes per CSS
   Display, `display: none` pruned.
3. **Layout** (`layout.rs`, `boxes.rs`) — inline formatting with line boxes
   in `layout.rs`; all box-level layout (block flow with margin collapsing,
   flex, floats, absolute positioning) through Taffy 0.14 in `boxes.rs`, with
   inline formatting contexts measured through Taffy's measure hooks.
4. **Paint** (`paint.rs`) — backgrounds, borders, and text into a
   `tiny-skia` pixmap.
5. **Encode** (`png.rs`) — one PNG per call.

## Prior art

- [NetSurf](https://www.netsurf-browser.org/) — `hubbub` parse, `libcss`
  cascade, its own layout/paint; a small engine that still renders real
  pages.
- [Dillo](https://dillo-browser.github.io/) — style -> layout -> canvas.
- [Obscura](https://github.com/h4ckf0r0day/obscura) — Rust engine with its
  own `obscura-render` CPU paint over Taffy.
- [Blitz](https://github.com/DioxusLabs/blitz) — the quality/size benchmark
  this crate is priced against.
- [Kitesurf](https://developers.cloudflare.com/browser-run/kitesurf/) —
  agent-first rendering trade-offs: structure over pixel fidelity.

## Non-goals

No compositor, no incremental relayout, no animations, no GPU. A screenshot
runs the pipeline once from the current DOM.
