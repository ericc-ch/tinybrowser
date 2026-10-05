# Handoff (2026-10-05)

Goal: Finish the Blitz 0.3.0-beta.2 cutover on `blitz-adopt`: Blitz owns parse/style/layout/paint; tinybrowser keeps only the JS bindings, observer journal, resource dials, and CDP surface, with pre-cutover leftovers deleted and rendering fidelity checked against Chromium.

Plan: Land forward commits on `blitz-adopt` (no history rewrite), verify each batch with `tools/check` and `cargo test --workspace`, record size and upstream Blitz limitations in `docs/progress.md`, and compare real pages against Chromium to find Blitz layout gaps.

State: HEAD `4ecd798`, clean tree. `tools/check` green and `cargo test --workspace` green (32 suites) on this tree; the `dom` crate is deleted from the workspace and `Cargo.lock`. `blitz-adopt` has no upstream tracking yet (not pushed). The GitHub fidelity gaps below are open; the WPT `dom/nodes` re-check and release-size re-measure are outstanding.

Done:

- `09e65e8`, `6109563`, `38b0cc9` — dead-shim deletion, element-check unification, stylesheet-pipeline removal, lifecycle split, settled screenshots, subresource byte sharing. Verified together at `38b0cc9`: `tools/check` exit 0 (`/tmp/opencode/check_final2.log`) and `cargo test --workspace` 36 suites green (`/tmp/opencode/test_final1.log`).
- `4ecd798` — `crates/dom` deleted (no dependents; workspace member and `Cargo.lock` pruned), `is_iframe_element` made AnonymousBlock-safe, `docs/progress.md` now lists every Blitz limitation found. Verified: `tools/check` exit 0 (`/tmp/opencode/check_commit1.log`), `cargo test --workspace` 32 suites green (`/tmp/opencode/test_commit2.log`).
- Earlier cutover commits (`78229e1`, `3e8087d`, `5b6ed2c`) are in history; their known-fails and the last recorded size live in `docs/progress.md`.

Unfinished:

- GitHub homepage rendering gaps vs Chromium (logged-out, 1280x800). Repro: `/tmp/opencode/pw/shot-tiny.cjs` (tinybrowser over CDP; run with `nix develop --command node`) plus the headless Chromium command in Gotchas. Artifacts: `/tmp/opencode/github-compare.png` (side-by-side), `github-tinybrowser.png`, `github-chromium-anon.png`. Symptoms: hero heading/subtitle shifted left and clipped instead of centered; "New rendering engine #920" illustration paints huge on the left overlapping the hero; bottom purple glow and Copilot mascots do not paint; hero vertical rhythm collapsed (flush under nav); heading falls back to a non-GitHub webfont. Resume: inspect the hero section (grid/centering and absolute placement) and gradient/image paint.
- `page.goto` to `https://github.com` over tinybrowser CDP times out waiting for `domcontentloaded` (90s) even though the page renders and `title()`/`url()` resolve; local pages fire it fine (repo Playwright suites pass). Check `DOMContentLoaded`/load-event emission for external pages (load-event gating in `document/mod.rs`).
- WPT `dom/nodes` regressions unverified since the cleanup commits. Comparing `/tmp/opencode/wpt-dom-nodes.json` (branch) vs `/tmp/opencode/wpt-dom-nodes-main.json` (main `b85926b`): 295 vs 278 fully-OK files, 28 fixes (mostly SVG/XML ERROR to OK), 11 OK-to-ERROR/TIMEOUT/CRASH:
  - `CharacterData-remove.html`, `MutationObserver-textContent.html`, `Node-compareDocumentPosition.html`, `Node-contains.html`, `Node-parentNode.html` (TIMEOUT), `Node-properties.html`, `ParentNode-children.html`, `ParentNode-querySelectorAll-removed-elements.html` (CRASH), `moveBefore/relevant-mutations.html` (TIMEOUT), `name-validation.html`, `node-realm-mixed-across-adoption.html`
  Logs predate `09e65e8`..`4ecd798`. Needs approval: retest just these files with `tools/wpt`.
- Release size not re-measured since `4ecd798`; `docs/progress.md` still records `11,985,160 bytes (2026-10-05)`. Run `nix develop --command ./tools/release` and update only the latest size, latest total, and scored groups.
- `subresource_bytes` on `Document` has no cap or eviction; cleared only on navigation.
- `blitz-adopt` is not pushed.

Next:

1. Fix the GitHub hero/illustration layout gaps; re-shoot with the same commands and diff against Chromium.
2. Investigate the missing `domcontentloaded` over CDP for external pages.
3. With explicit approval, retest the 11 WPT files listed above and refresh the branch-vs-main comparison.
4. Re-measure the release binary; update `docs/progress.md` size lines.
5. Decide `subresource_bytes` cap/eviction; push `blitz-adopt`.

Decisions made:

- Upstream Blitz bugs are not worked around; they fail closed as known-fails recorded in `docs/progress.md` (resolve_url panic, no PI/CDATA/doctype kinds, no shadow DOM, no template contents, no form-control state, no image-element state, no visibility helper, hardcoded NoQuirks styling, AnonymousBlock in the tree, no per-element scroll API, always scripting-disabled parse).
- Deletions land as forward commits, never revert/reset.
- The JS-visible `<img>` surface stays on our dial + decode because Blitz exposes no image-element API; Blitz fetches share raw bytes through `subresource_bytes`.
- Journal ordering is per-document; the stylesheet pipeline is deleted (Blitz fetches sheets itself); protocol `Stylesheet` variants are kept for carrier compatibility.
- `crates/dom` had no dependents and was deleted outright.

Gotchas:

- Run Cargo and all runners via `nix develop --command`; the host shell lacks `pkg-config`/OpenSSL.
- `/mnt/ssd/rust-targets` fills up; a link failure with "No space left" is disk, not code (`cargo clean -p tinybrowser` recovered about 70G this session).
- WPT/Playwright/CDP suite runs need explicit approval and must stay surgical.
- Live Chromium comparisons use the `playwriter` CLI (session 1, extension-connected). The ms-playwright cached chrome cannot run here (missing `libglib-2.0.so.0`). Logged-out headless shots:
  `/etc/profiles/per-user/erickc/bin/google-chrome-dev --headless=new --no-sandbox --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --user-data-dir=/tmp/opencode/chrome-anon --window-size=1280,800 --virtual-time-budget=15000 --screenshot=<abs.png> <url>`
- The debug binary for shots is `/mnt/ssd/rust-targets/shared/debug/tinybrowser` (rebuilt by `cargo test`); `tools/binary-path` maps profile to path. tinybrowser honors CDP `setViewportSize` (shots came out 1280x800).
- `/tmp/opencode` artifacts are ephemeral; copy the comparison PNGs if they must survive a reboot.
- This worktree: `/home/erickc/.local/share/opencode/worktree/174cc2/swift-sailor`; main worktree is `~/projects/tinybrowser` (`main` at `b85926b`).
