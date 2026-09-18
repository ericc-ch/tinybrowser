# render

One-shot HTML/CSS rendering for screenshots: style, layout, paint, PNG.

This crate exists because the measured Blitz stack costs +6.6 MB against the
shipping binary (`docs/researches/size-budget.md`), which does not fit the
10 MB ceiling. The alternative shape is the one small browsers have always
used: a CSS subset over the DOM we already have, a box tree, and a CPU
rasterizer.

## Pipeline

1. **Style** (`stylo.rs`, `stylo_view.rs`, `stylo_map.rs`, `style.rs`) —
   one-shot styling through Servo's Stylo engine: parse `<style>` text and
   fetched sheets, match selectors, cascade and inherit to a
   `ComputedValues` per element, then map the properties the layout engine
   implements into the small computed `Style` in `style.rs`. Cascade,
   selector matching, and the property database follow the CSS specs; the
   mapping keeps the documented subset behavior.
2. **Box tree** (`tree.rs`) — anonymous block and inline boxes per CSS
   Display, `display: none` pruned.
3. **Layout** (`layout.rs`, `boxes.rs`) — inline formatting with Parley
   shaping and UAX#14 line breaking in `layout.rs`; all box-level layout
   (block flow with margin collapsing, flex, grid, floats, absolute
   positioning) through Taffy 0.14 in `boxes.rs`, with inline formatting
   contexts measured through Taffy's measure hooks.
4. **Paint** (`paint.rs`) — backgrounds and borders into a `tiny-skia`
   pixmap; shaped glyph IDs rasterize through `skrifa` outlines.
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
