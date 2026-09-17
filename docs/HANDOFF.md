# Handoff (2026-09-18)

State: working tree on `main` adds always-in screenshot rendering. Gates on
this tree: `tools/ub lint` (clippy, workspace, all targets), `cargo test
--workspace` (30 suites), `tools/playwright/run` (7 passed), `tools/cdp/run`
(Blink corpus 1/1), `tools/wpt/score webmessaging/` (106/136, unchanged), and
release binary **6,606,240 bytes** (3.4 MB under the cap). Screenshots ride
CDP `Page.captureScreenshot` and WebDriver `GET /session/{id}/screenshot`.

## What shipped

- **`crates/render`**: one-shot pipeline from `dom::Dom` to PNG. CSS parse and
  cascade (`cssparser` through the DOM's selector engine), UA stylesheet,
  `@media` width queries, block/inline/flex formatting, `tiny-skia` paint,
  `fontdue` text over an embedded Liberation Sans subset (OFL-1.1), `png`
  encode. Module docs cite the specs; the README lists the prior art
  (`NetSurf`, Dillo, Obscura, Blitz, Kitesurf) and the non-goals.
- **`dom`**: `Dom::compile_selectors` + `CompiledSelectors::matching_specificity`
  compile a style rule once and match many elements without recompiling
  (`crates/dom/src/select.rs`).
- **IPC**: `Command::Screenshot { frame, request }` and `Reply::Screenshot`;
  the PNG streams browser-ward as bounded body frames on the same request id
  (no base64 through the control plane). `PROTOCOL_VERSION` is 4. The three
  reply maps are grouped in `link::Waiters`.
- **Renderer**: `Engine::screenshot_frame` reads `<style>` and loaded
  `<link rel=stylesheet>` sheets, renders, and crops to the caller's clip; the
  800x600 viewport constants are shared with the JS bindings
  (`innerWidth`/`innerHeight`).
- **Stylesheets**: `<link rel=stylesheet>` dials are queued when the parser
  finishes and delay the load event
  (<https://html.spec.whatwg.org/multipage/links.html#link-type-stylesheet>),
  so `Page.navigate` resolving on load means the sheet is in before a
  screenshot. Failures drop the sheet without holding load. `DialKind` gained
  `Stylesheet` and the wasm WIT `fetch-kind` mirrors it.
- **CDP**: `Page.captureScreenshot` (clip-aware), `Page.getLayoutMetrics`
  (the three viewport shapes Playwright dereferences),
  `Emulation.clearDeviceMetricsOverride`. `Runtime.callFunctionOn` now honors
  `awaitPromise` for handle calls, which Playwright's screenshot preparation
  needs (`evaluateHandle` over a poller promise).
- **WebDriver**: W3C Take Screenshot route returning base64 PNG.
- **Tests**: `crates/render/tests/render.rs` pipeline smoke tests,
  `tools/playwright/screenshot.spec.ts` (pixel probes through a dependency-free
  PNG reader in `tools/playwright/png.ts`), and a WebDriver screenshot test in
  `tests/webdriver.rs`.

## Limits (deliberate, documented in the crate)

No images, no floats/absolute positioning/grid/tables, no complex-script text
shaping, no per-element or `fullPage` layout metrics (`contentSize` reports
the viewport), no device scale factor. Screenshots capture the viewport at
800x600 unless the caller's clip asks for a larger one. Vertical margin
collapsing is sibling-only, and `getComputedStyle`-style queries do not exist
yet.

## Next, in order

1. Images: PNG decode via the `png` crate (already shipped) painted into
   `LayoutBox` replaced boxes; JPEG later.
2. Absolute/relative positioning and `border-radius`/opacity, then more CSS
   (grid, tables) as real fixtures demand.
3. `Page.getLayoutMetrics.contentSize` from the real layout height so
   `fullPage` screenshots capture the document.
4. Element screenshots (`Page.captureScreenshot` with a node clip) once
   layout rects are queryable through the protocol.

## Gotchas

- One WPT run at a time: the wrapper locks the shared venv.
- The clipboard/IPC version is 4; a mismatched renderer child fails the
  handshake, not serde.
- Playwright always sends `clip` with `scale`; the renderer crops but ignores
  non-unit scale.
- Scratch WPT files must be deleted before committing; commits are unsigned.
