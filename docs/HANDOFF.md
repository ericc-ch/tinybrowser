# Handoff (2026-09-13)

State: `spike/blitz-screenshot` (commit `6b43b1b`) holds a working Blitz
one-shot screenshot pipeline: standalone probe plus a feature-gated CLI
command, both rendering the same byte-identical PNG. `docs/researches/size-budget.md`
prices the stack above the 10 MB ceiling (layout alone busts it); no budget
decision has been made and the branch is not merge-ready. Verification is
green: feature-off `cargo check --release`, `cargo clippy --release
--features screenshot`, `cargo fmt --all`, probe and CLI renders.

Done:

- Blitz screenshot spike, commit `6b43b1b`: `spikes/blitz-screenshot`
  renders `test-page.html` in ~20 ms and the CLI (`cargo build --release
  --bin tinybrowser --features screenshot`) produced a byte-identical file
  (`cmp` clean against the probe output).
- Size ladder in `docs/researches/size-budget.md` (spike section + trim
  follow-up), measured with `stat` on tuned-profile builds: probe empty
  289,256 B, layout 5,602,832 B, full 7,885,192 B; CLI baseline 5,810,304 B,
  layout-only 10,788,304 B, full 13,113,008 B, full+abort 11,922,640 B.
- Non-packing trims measured: abort −1,190,368 B; non-PIE −574,240 B; lld
  ICF −107,392 B (probe); fontdue isolated at +116 KB vs the Parley stack.
- Dev shell toolchain, commit `6b43b1b`: `python3`, `cargo-bloat`, `lld`,
  `upx`, `xz`, `binutils`, verified by running each `--version` in
  `nix develop` (flake.nix, nativeBuildInputs).
- Obsidian, outside the repo: `projects/tinybrowser/screenshot.md` updated
  with the spike result and trim levers.

In flight:

- Budget decision: 10 MB needs abort + non-PIE + a tiny-skia paint backend +
  fontdue + ICF (~9.7 MB, knife-edge). Alternatives: raise the ceiling to
  ~12 MB, or Taffy + own cascade for agent-UI pages.
- The `screenshot` feature is scaffolding; intent is baked-in default once
  the direction is picked.
- Commits are unsigned (keyring agent cannot sign in-session). Re-sign later
  with `git rebase -x 'git commit --amend -S' main` when the agent works.

Next:

1. Pick the budget path. If trims: implement the anyrender `PaintScene`
   backend over tiny-skia first (replaces vello_cpu, ~1.0–1.2 MB).
2. Then replace the Parley stack with fontdue + own line breaking (~0.57 MB).
3. If the ceiling moves instead: drop the feature gate, make `screenshot`
   a normal CLI command, and wire `CaptureScreenshot` to it.

Decisions made:

- Blitz stays out of the default binary until the budget call.
- Probe lives under `spikes/` (workspace-excluded) so shipping size and the
  root lockfile stay measurable.
- vello_cpu for the first paint backend; tiny-skia remains the locked target.

Gotchas:

- `panic=abort` conflicts with rquickjs `catch_unwind`: JS-op panics would
  kill the renderer process instead of raising a JS error.
- `relocation-model=static` drops ASLR; unsafe for the renderer path.
- `image` codecs and `force-unwind-tables=no` are measured dead ends.
- UPX `--lzma` adds ~183 ms per process start; default NRV ~33 ms.
