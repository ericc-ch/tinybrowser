# Handoff: canvas review follow-ups

## State

- PR #121 (`feature/canvas-cleanup`) is merged into `main`: `72ace510`
- Review-fix commits: `dc2ef456`, `b79ed5bc`
- Verification passed: `cargo check --workspace`, `cargo test --workspace`, `cargo clippy -p renderer --all-targets -- -D warnings`, `cargo fmt --all -- --check`
- Full `html/canvas` WPT: 3401 tests, 2129 pass, 210 unexpected, 31 timeout, 1031 error, 4489 subpass, 1318 subfail (same top-level pass count as baseline)

Known accepted differences are recorded in the PR comment below. The important ones are the intentional `2d.video.invalid` failure after removing the fake media shim, one worker `convertToBlob` subtest blocked on worker runtime, tests that previously timed out and now run, and a flaky `variationSelectors` failure that also reproduces on the baseline worktree.

## Follow-ups

The detailed review response is the PR comment:
https://github.com/ericc-ch/tinybrowser/pull/121#issuecomment-6332345285

The remaining work, in dependency order:

1. **OffscreenCanvas as a Rust canvas host keyed by bitmap id.** This is the worker blocker. It replaces the hidden `<canvas>` shim, keys the registry by a bitmap id, and gives `<canvas>` and `OffscreenCanvas` a shared host trait.
2. **Rust-owned `TextMetrics` storing `ShapedText`.** Removes the `__tb*` text state, the repeated shaping, and the `TextCluster` brand gap.
3. **Unified CSS `@font-face` and script `FontFace` loading.** One loader and one `FontFaceSet`, including status, loading events, and the ready promise.
4. **Replace the hand-written color parser with `cssparser-color`.** Carry `AbsoluteColor` through parsing, relative colors, `color-mix`, and serialization.
5. **Filter bounds, draw order, and single blend composite.** Bound per-draw filters to the device rect, apply the filter before shadow/globalAlpha/composite, and composite the layer once.

Secondary items already listed in the PR comment: SVG raster cache and destination-scale rasterization, font bytes as `ArrayBuffer`, and MessagePort/buffer detach ordering.

## Next move

Start with item 1 on a new branch. Do not bundle it with items 2–5. Each item needs its own WPT before/after parity run; a mixed branch makes regressions untraceable.

## Suggested skills

- Use `improve-codebase` for the OffscreenCanvas host and TextMetrics ownership changes.
- Use `code-review` before each follow-up PR.
- Use `technical-writing` for updates to this file or `docs/priorities.md`.

## References

- PR #121: https://github.com/ericc-ch/tinybrowser/pull/121
- Review response: https://github.com/ericc-ch/tinybrowser/pull/121#issuecomment-6332345285
- Baseline WPT report: `/tmp/opencode/canvas-before-report.json`
- Final WPT report: `/tmp/opencode/canvas-after2-report.json`
- Existing priorities: `docs/priorities.md`
- Existing known gaps: `docs/upstream.md`
