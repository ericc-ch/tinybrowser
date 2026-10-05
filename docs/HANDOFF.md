# Handoff (2026-10-05)

Goal: Finish the Blitz 0.3.0-beta.2 cutover on `blitz-adopt`: Blitz owns parse/style/layout/paint; tinybrowser keeps only the JS bindings, observer journal, resource dials, and CDP surface, with pre-cutover leftovers deleted and rendering fidelity checked against Chromium.

Plan: Land forward commits on `blitz-adopt` (no history rewrite), verify each batch with `tools/check` and `cargo test --workspace`, record size and upstream Blitz limitations in `docs/progress.md`, and compare real pages against Chromium to find gaps.

State: clean tree, branch `blitz-adopt` two commits ahead of the PR #41 merge `6ab35db` (`b59a052` review fixes + this handoff), not pushed. PR #41 (plain non-blocking logger) is merged as `6ab35db`; the review commits were rebased on top. The code-review findings from `9414c02..4013b20` are fixed. Gates on this revision: `tools/check` green, `cargo test --workspace` green, full Playwright gate 39 passed / 4 failed (all four are pre-existing form gaps listed below, not from this round). WPT `dom/nodes` retest, release-size re-measure, and push are outstanding.

Done (this round, `b59a052`):

- Single image decode: our `<img>` dial and `render/decode.rs` (zune-jpeg, image-webp, 32 MiB pixel budget) are deleted. Blitz's fetch is the only fetch; `<img>` requests are waiters settled from Blitz's public `ElementData::image_data` on each delivery (`load` with natural dims, `error` otherwise). SVG natural size is declared absolute width/height else the 300x150 default object size (Chromium answers 300x150 even for viewBox-only SVG, probed 2026-10-06); SVG and GIF `<img>` now load instead of erroring.
- `6ab35db` — merged PR #41: the logging crate is a plain non-blocking logger (`logging::install(level, file)`; `Config`/`Logger`/`ParseLevelError` deleted). Call sites keep using the macros; `src/main.rs` and `tests/renderer_process.rs` were updated by the PR. Rebased the review commits on top.

- Init scripts are tab state: `Tab.init_scripts` with caller-assigned ids, `Mount`/`ResponseStart` replay the list on every renderer acquisition, every frame's world shares it (parsed and script-created child frames run it), `runImmediately` evaluates in existing frames, `remove` stops future documents. `worldName` is accepted but runs in the main world (no isolated worlds; Playwright's utility registration uses an empty source).
- Viewport: `mount_virtual` replays the tab's viewport and scripts; child frames inherit the tab size; `Emulation.setDeviceMetricsOverride` requires integer sides, `0` clears, `>4096` errors, `mobile`/`deviceScaleFactor != 1` are refused; one `renderer::DEFAULT_VIEWPORT`. The world stores the persistent size so `window` metrics survive renderer swaps.
- Capture: the renderer applies `ScreenshotRequest` dimensions for the capture only and restores the emulated viewport; raw CDP clips beyond the viewport work; `Page.getLayoutMetrics` reports real scrollable `contentSize` (2051px on the tall fixture); CSSOM View `client`/`scroll`/`offset` boxes are implemented through the generated `Element`/`HTMLElement` contracts and flush layout on read, so Playwright `fullPage` sizes correctly.
- Input selection applicability matches the text-like states: `<input type=email>` answers `null` for `selectionStart/End` and rejects `setSelectionRange`, matching Chromium; this fixed the Playwright `fill` retry loop on email fields.
- CDP lifecycle: the tab's current `loaderId` is kept for lifecycle events, `Page.setLifecycleEventsEnabled` gates `Page.lifecycleEvent` (instead of no-op), `init` is emitted on commit, and the `about:blank` path fires `DOMContentLoaded` before `load`.
- Canvas `width`/`height` follow the HTML parsing rules (ASCII whitespace, digit prefix, u32 wrap; Chromium's overflow-to-default divergence noted in code); `getContext` answers spec-`null` with the deviation spelled out.
- Bindgen: optional `any` with `= null` supplies `null`, not `undefined`; the dead `any`-attribute getter arm is removed.
- Renderer: `load` waits for Blitz critical resources (stylesheets) and rechecks on each delivery; failed and cached subresource deliveries settle layout; a realm born while a response streams defers init scripts and rebinds `document` after install.
- Tests: `tools/playwright/{init-scripts,viewport}.spec.ts`, clip/fullPage and stylesheet-load cases, raw-CDP helper (`tools/playwright/cdp.ts`), wire round-trips for the new commands, and CDP helper unit tests.

Unfinished:

- Playwright gate is 39/43. The four failures predate this round (their code paths were untouched by it):
  - `<select>` gets a 0x0 box from Blitz, so Playwright's visible check fails (`forms.spec.ts:8`, `forms.spec.ts:52`, `interaction.spec.ts:45`). Upstream Blitz layout gap.
  - `keyboard.type` into `<textarea>` is lost (`interaction.spec.ts:28`): `__tbSetNativeValue` writes the `value` attribute, while textarea `.value` reads its child text. Needs a real API-value store in the bindings.
- `Target.attachToTarget` is not implemented, so Playwright `context.newCDPSession` fails; the new specs use the raw `/json/list` WebSocket helper instead. Real gap for CDP clients.
- Isolated worlds are not implemented; `worldName` scripts execute in the main world.
- GitHub hero WebGL glow/mascots: page requests `getContext('webgl')`, we answer `null` (spec), so `.lp-IntroVisuals-canvas` never paints. Real support needs a WebGL stack; known-fail.
- `document.styleSheets` / `document.fonts` are still undefined (real `StyleSheetList`/`FontFaceSet` over Blitz needed); `document.images`/`document.scripts` absent (cheap `HTMLCollection`s).
- GitHub heading wraps greedily where Chromium balances two lines (`text-wrap: balance` unsupported).
- WPT `dom/nodes` regressions unverified since the cleanup commits (11 OK-to-ERROR/TIMEOUT/CRASH files listed in the previous handoff). Needs approval: retest just those files with `tools/wpt`.
- Release size not re-measured since `4ecd798`; `docs/progress.md` still records `11,985,160 bytes (2026-10-05)`. Run `nix develop --command ./tools/release` and update only the latest size, latest total, and scored groups.
- `subresource_bytes` on `Document` has no cap or eviction; cleared only on navigation.

Next:

1. Fix the textarea API-value store and check whether a UA/Blitz-side change can give `<select>` a box (or record it as an upstream known-fail).
2. With explicit approval, retest the 11 WPT files listed in the previous handoff and refresh the branch-vs-main comparison.
3. Re-measure the release binary; update `docs/progress.md` size lines.
4. Decide `subresource_bytes` cap/eviction; push `blitz-adopt`.
5. Optional: implement `Target.attachToTarget` so `context.newCDPSession` works.

Decisions made:

- Upstream Blitz bugs are not worked around; known-fails live in `docs/progress.md` (resolve_url panic, no PI/CDATA/doctype kinds, no shadow DOM, no template contents, no form-control state, no image-element state, no visibility helper, hardcoded NoQuirks, AnonymousBlock in tree, no per-element scroll API, always scripting-disabled parse).
- Deletions land as forward commits; bindings follow the pinned IDL, never the reverse.
- Emulation is tab state; screenshots apply a capture-scoped size and restore the emulated viewport.
- Init scripts are tab state and replay through mounts; `worldName` is accepted because rejecting it would break Playwright's empty-source utility registration, and isolated worlds do not exist.
- CSSOM View box reads flush layout (`base.resolve`) like the existing rect paths.
- No Blitz fork: every GitHub divergence traced back to our CDP/bindings layers.

Gotchas:

- Run Cargo and all runners via `nix develop --command`; the host shell lacks `pkg-config`/OpenSSL.
- The machine-wide `~/.cargo/config.toml` points every project at `/mnt/ssd/rust-targets/shared`, which this session found cross-contaminated between worktrees. Use an isolated dir for reliable builds, e.g. `CARGO_TARGET_DIR=/mnt/ssd/rust-targets/blitz-adopt`.
- `/mnt/ssd` fills up; a link failure with "No space left" is disk, not code.
- WPT/Playwright/CDP suite runs need explicit approval and must stay surgical. The full Playwright gate takes under a minute once `node_modules` exists.
- Scratch probes that open a raw CDP WebSocket can hang on exit; prefer `tools/playwright/cdp.ts` inside the committed specs.
- Live Chromium comparisons use the `playwriter` CLI (session 1, extension-connected), e.g. `playwriter -s 1 -f /tmp/opencode/eval-email2.js`. The ms-playwright cached chrome cannot run here (missing `libglib-2.0.so.0`). Headless shots:
  `/etc/profiles/per-user/erickc/bin/google-chrome-dev --headless=new --no-sandbox --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --user-data-dir=/tmp/opencode/chrome-anon --window-size=1280,800 --virtual-time-budget=15000 --screenshot=<abs.png> <url>`
- This worktree: `/home/erickc/.local/share/opencode/worktree/174cc2/swift-sailor`; main worktree is `~/projects/tinybrowser` (`main` at `b85926b`).
