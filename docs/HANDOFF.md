# Handoff (2026-10-05)

Goal: Finish the Blitz 0.3.0-beta.2 cutover on `blitz-adopt`: Blitz owns parse/style/layout/paint; tinybrowser keeps only the JS bindings, observer journal, resource dials, and CDP surface, with pre-cutover leftovers deleted and rendering fidelity checked against Chromium.

Plan: Land forward commits on `blitz-adopt` (no history rewrite), verify each batch with `tools/check` and `cargo test --workspace`, record size and upstream Blitz limitations in `docs/progress.md`, and compare real pages against Chromium to find gaps.

State: HEAD `d2f9e1b`, clean tree. Workspace check green and functional integration pass (see Done). The GitHub homepage renders the correct hero at 1280x800; the remaining visible gap is the WebGL glow/mascots (known-fail). WPT `dom/nodes` re-check and release-size re-measure are outstanding. `blitz-adopt` is not pushed.

Done:

- `0dfe1fe` — real `DOMContentLoaded` over CDP (`Page.lifecycleEvent` + `Page.domContentEventFired`), persistent `Emulation.setDeviceMetricsOverride` viewport (mount-carried across renderer restarts, live `innerWidth/innerHeight/outerWidth/outerHeight` getters, `Page.getLayoutMetrics` reports the emulated size, screenshots paint the document viewport), real `Page.addScriptToEvaluateOnNewDocument`/`removeScriptToEvaluateOnNewDocument`, re-resolve on every subresource delivery, documents born at a real viewport (root/body no longer 0-width). Verified: `/tmp/opencode/units12b.out` (goto DCL 148ms, addInitScript=42, 100vw layout follows 1024x768, screenshot 1024x768, revert) and `/tmp/opencode/canvas-verify3.out` (`ALL PASS`).
- `d2f9e1b` — webidl-bindgen supports IDL `any` and union returns (`RenderingContext?`), canvas `getContext` answers spec-`null` with `width`/`height` reflection. Verified: `cargo test -p webidl-bindgen` green; integration canvas check (`{"getContextType":"function","width":300,"height":150,"ctx2d":"null","webgl":"null"}`); GitHub probe `protoGetContext:"function"`, hero `h1Rect [178,164,924,138]`, `bodyRect [0,0,1280,11268]`; screenshot `/tmp/opencode/github-tinybrowser-v4.png`.
- Earlier: `4ecd798` (crates/dom deleted; `docs/progress.md` Blitz limitations) verified by `tools/check` + 32 test suites at the time; `09e65e8`, `6109563`, `38b0cc9` (dead shims, lifecycle split, subresource byte sharing) verified by `tools/check` + 36 suites at `38b0cc9`.

Unfinished:

- GitHub hero WebGL glow/mascots: page requests `getContext('webgl')`, we answer `null` (spec), so `.lp-IntroVisuals-canvas` never paints. Real support needs a WebGL stack; known-fail.
- `document.styleSheets` / `document.fonts` are still undefined (JS API gaps, not shims). `styleSheets` needs a real `StyleSheetList`/`CSSOM` surface over Blitz's `author_stylesheets()`; `fonts` needs a real `FontFaceSet` over Blitz's font loading. `document.images`/`document.scripts` are also absent (plain `HTMLCollection`s, cheap to add).
- GitHub heading wraps greedily ("…happens / together") where Chromium balances two lines; `text-wrap: balance` is unsupported. Heading also renders without the opsz-axis tuning Chromium applies.
- WPT `dom/nodes` regressions unverified since the cleanup commits. Comparing `/tmp/opencode/wpt-dom-nodes.json` (branch) vs `/tmp/opencode/wpt-dom-nodes-main.json` (main `b85926b`): 295 vs 278 fully-OK files, 28 fixes, 11 OK-to-ERROR/TIMEOUT/CRASH: `CharacterData-remove.html`, `MutationObserver-textContent.html`, `Node-compareDocumentPosition.html`, `Node-contains.html`, `Node-parentNode.html`, `Node-properties.html`, `ParentNode-children.html`, `ParentNode-querySelectorAll-removed-elements.html`, `moveBefore/relevant-mutations.html`, `name-validation.html`, `node-realm-mixed-across-adoption.html`. Logs predate `09e65e8`..`d2f9e1b`. Needs approval: retest just these files with `tools/wpt`.
- Release size not re-measured since `4ecd798`; `docs/progress.md` still records `11,985,160 bytes (2026-10-05)`. Run `nix develop --command ./tools/release` and update only the latest size, latest total, and scored groups.
- `subresource_bytes` on `Document` has no cap or eviction; cleared only on navigation.
- `blitz-adopt` is not pushed.

Next:

1. Add the cheap real collections (`document.images`, `document.scripts`); decide and scope real `styleSheets`/`fonts`.
2. With explicit approval, retest the 11 WPT files listed above and refresh the branch-vs-main comparison.
3. Re-measure the release binary; update `docs/progress.md` size lines.
4. Decide `subresource_bytes` cap/eviction; push `blitz-adopt`.
5. Optional fidelity: check whether the font is actually fetched/registered (glyph/axis diff vs Chromium) before treating it as a gap.

Decisions made:

- Upstream Blitz bugs are not worked around; known-fails live in `docs/progress.md` (resolve_url panic, no PI/CDATA/doctype kinds, no shadow DOM, no template contents, no form-control state, no image-element state, no visibility helper, hardcoded NoQuirks, AnonymousBlock in tree, no per-element scroll API, always scripting-disabled parse).
- Deletions land as forward commits; bindings follow the pinned IDL, never the reverse. IDL `any`/union returns were added to webidl-bindgen rather than worked around, and canvas answers spec-`null` because no rasterizer/GL stack exists.
- Emulation is tab state: `Emulation.setDeviceMetricsOverride` persists, mounts carry the viewport, and screenshots paint the document viewport (no per-capture override).
- A stylesheet or image arrival re-resolves the Blitz tree immediately, so capture never races a partial cascade.
- The JS-visible `<img>` surface stays on our dial + decode (Blitz exposes no image-element API); raw bytes share through `subresource_bytes`.

Gotchas:

- Run Cargo and all runners via `nix develop --command`; the host shell lacks `pkg-config`/OpenSSL.
- The machine-wide `~/.cargo/config.toml` points every project at `/mnt/ssd/rust-targets/shared`, which this session found cross-contaminated between worktrees (stale `dom` bindgen output, a main-tree `logging` rlib linked against this tree's `main.rs`). Use an isolated dir for reliable builds, e.g. `CARGO_TARGET_DIR=/mnt/ssd/rust-targets/blitz-adopt` (works for `tools/check` and `cargo`; `tools/binary-path` follows it).
- `/mnt/ssd` fills up; a link failure with "No space left" is disk, not code (`cargo clean -p tinybrowser` recovered ~70G earlier).
- WPT/Playwright/CDP suite runs need explicit approval and must stay surgical.
- Live Chromium comparisons use the `playwriter` CLI (session 1, extension-connected). The ms-playwright cached chrome cannot run here (missing `libglib-2.0.so.0`). Logged-out headless shots:
  `/etc/profiles/per-user/erickc/bin/google-chrome-dev --headless=new --no-sandbox --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --user-data-dir=/tmp/opencode/chrome-anon --window-size=1280,800 --virtual-time-budget=15000 --screenshot=<abs.png> <url>`
- Repro scripts: `/tmp/opencode/pw/{test-units12,shot-tiny,probe4-tiny,fixed-repro,viewport-check}.cjs`; artifacts under `/tmp/opencode/` (`units12b.out`, `canvas-verify3.out`, `github-tinybrowser-v4.png`, `github-probe*.json`).
- This worktree: `/home/erickc/.local/share/opencode/worktree/174cc2/swift-sailor`; main worktree is `~/projects/tinybrowser` (`main` at `b85926b`).
