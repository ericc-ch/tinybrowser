# Handoff (2026-09-18)

State: branch `chase/screenshot`, rebased onto `chase/size-flags`. The size
slice underneath brought the hand-rolled CLI (no clap), hyper-direct servers
(no axum), and linker/C-flag knobs; this branch adds the Stylo cascade and
the WPT reftest executor. Gates on this tree: `tools/ub lint` (clippy,
workspace, all targets), `cargo test --workspace` (the workspace suite is green),
`tools/playwright/run` (7 passed), `tools/wpt/score css/css-color/ --
--test-types reftest` (266/307), `tools/ub valgrind render` (0 errors; the
only leaks are Stylo's intentionally leaked thread-local caches), and release
binary **9,530,704 bytes** (469 KB under the cap). Screenshots ride CDP
`Page.captureScreenshot` and WebDriver `GET /session/{id}/screenshot`.

## What shipped

- **`crates/render`**: one-shot pipeline from `dom::Dom` to PNG. Styling runs
  through Servo's Stylo (`stylo.rs`, `stylo_view.rs`, `stylo_map.rs`):
  selector matching, inheritance, and the full property database, mapped to
  the layout model the engine implements. Taffy 0.14 box layout (block flow
  with margin collapsing, flex, grid, floats, absolute positioning) with
  inline formatting contexts measured through Taffy's measure hooks, Parley
  text shaping with `skrifa` outlines painted by `tiny-skia`, `png` encode.
  Module docs cite the specs; the README lists the prior art (`NetSurf`,
  Dillo, Obscura, Blitz, Kitesurf) and the non-goals.
- **`dom`**: the selector engine (`crates/dom/src/select.rs`, `cssparser` +
  `selectors`) now serves only the JS `querySelector`/`matches` bindings; the
  render cascade no longer uses it.
- **WPT**: the product registers a `reftest` executor
  (`tools/wpt/tinybrowser_wpt.py`), so CSS reftests compare screenshots
  through WebDriver. Two renderer fixes came out of the first run:
  `stylo_map` resolves `currentcolor`, `color-mix()`, relative color syntax,
  and `contrast-color()` against the element's own color
  (<https://drafts.csswg.org/css-color-5/#resolving-color-values>), and
  `collect_stylesheets` strips the `<![CDATA[` / `]]>` wrapper of XHTML
  `<style>` elements (WPT serves `.xht` as XML). The JS globals gained
  `outerWidth`/`outerHeight` (no chrome, so they equal the 800x600 viewport),
  which the reftest executor reads.
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

No images, no tables, no complex-script verification yet (shaping runs, but
only Latin coverage is tested), no `@font-face` web fonts (the embedded
subset is the only family), `media` attributes on `<link>`/`<style>` are
ignored (every sheet applies as screen), text does not wrap around floats
yet, no per-element or `fullPage` layout metrics (`contentSize` reports the
viewport), no device scale factor. Screenshots capture the viewport at
800x600 unless the caller's clip asks for a larger one.
`getComputedStyle`-style queries do not exist yet. The cascade is Stylo's
full property database, but `stylo_map` only forwards the properties the
box tree, Taffy, Parley, and paint implement.

## Next, in order

1. `opacity` (paint layers): the biggest `css/css-color` cluster (~11 tests)
   and the base for filters and stacking contexts later.
2. CSS conformance: widen the reftest baseline (`css/css-backgrounds/`,
   `css/css-display/`, `css/css-text/`) and fix clusters as they appear.
   Known color clusters: out-of-gamut clamping (`lch-009/010`,
   `oklch-009/010`), `hsla()` compositing, `@color-profile`, `color-mix()`.
3. Web fonts (`@font-face`): WPT text reftests rely on the Ahem test font,
   which is a `@font-face` resource today never fetched or registered.
4. Images: PNG decode painted into `LayoutBox` replaced boxes; JPEG later.
5. `border-radius`/gradients/stacking order, then more CSS as real fixtures
   demand.
6. `Page.getLayoutMetrics.contentSize` from the real layout height so
   `fullPage` screenshots capture the document.
7. Element screenshots (`Page.captureScreenshot` with a node clip) once
   layout rects are queryable through the protocol.

## Gotchas

- One WPT run at a time: the wrapper locks the shared venv.
- The child/IPC version is 4; a mismatched renderer child fails the
  handshake, not serde.
- Playwright always sends `clip` with `scale`; the renderer crops but ignores
  non-unit scale.
- Scratch WPT files must be deleted before committing. Commits before the
  Stylo slice are unsigned; this branch's new commits are signed.
- Stylo is MPL-2.0: distributing binaries needs the license and source
  notice, which this repo does not carry yet.
- Stylo gates `display: grid` behind `layout.grid.enabled`; the cascade sets
  that pref (and `layout.unimplemented`) before building the `Stylist`.
- Reftests run at the fixed 800x600 virtual viewport: the engine has no
  chrome, so `outerWidth`/`outerHeight` equal the inner size and
  `set window rect` only moves the stored rectangle.
- `.xht` pages are parsed as HTML; `collect_stylesheets` strips the CDATA
  wrapper so their `<style>` text still parses.

