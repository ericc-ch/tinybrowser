We are building the smallest, lightest, and fastest headless browser for AI agents.
Maximize web spec conformance per byte.

- WHATWG specs are ground truth. Start every implementation and review from the spec. Follow exact algorithm order and cite the spec URL at the implementation site.
- Heavily reference mature engines. Read Chromium/Firefox source code (see `docs/engines.md`). Never clone the repositories. When the spec and a shipped browser disagree, state the disagreement in a code comment and follow the browser that matches the spec algorithm.
- Breaking changes, churn, and rewrites are always welcome. When code fights you, zoom out and fix the architectural design. Do not work or patch around it.
- No lint silencing. Fix the underlying design flaw instead of suppressing compiler or lint diagnostics.
- Handwritten `unsafe` is prohibited unless runtime safety is proven (`// SAFETY:` invariant, Miri/valgrind checks via `tools/check`) and no safe alternative is as simple, clear, or fast.
- Let upstream bugs fail honestly. Never build tinybrowser workarounds for upstream engine bugs; record gaps in `docs/upstream.md`.
- Keep feedback loop fast. Cargo tests cover tinybrowser internals only (`cargo test --workspace`); never test web-platform spec conformance in cargo tests. Run the smallest slice that answers the question (single files or directories, never full suites). Never run long or whole-suite test runs without explicit approval.
- A WPT timeout is a failure. The harness budget (`DEFAULT_TIMEOUT = 3s`, `LONG_TIMEOUT = 5s`, `EXTRA_TIMEOUT = 2s` in `tools/wpt/tinybrowser_wpt.py`) is the speed standard, so a test that cannot finish inside it is too slow and is a work item. Profile and fix the slow path; never raise a budget to hide one.
- Commit messages should also record decisions.
- Throwaway experiments belong in `/tmp/`.

## Size and release

- Size is a headline constraint, but never pay with a used feature. `display: grid`, for example, stays: real pages use it, so grid layout code stays. Cut code nothing calls, not conformance.
- No binary packing. No compressor, stub, or self-extractor step (nothing UPX-like). Shrink real code, then strip what no consumer reads.
- The shipped artifact is `cargo build --release`. There is no post-link step (`tools/release` was deleted once the compiler could emit no unwind tables). Any future post-link step must work on a copy, smoke-test it, then replace atomically.
- The toolchain is pinned nightly (`rust-toolchain.toml`) for `[profile.release] rustflags` and in-tree std. Release invariants live in `Cargo.toml`/`build.rs` with their size reason: `panic = "abort"`, `force-unwind-tables=no`, `[unstable] build-std`. Changing one means re-measuring; see `docs/size.md`.
- Release never unwinds. Design for it: no cleanup may depend on `Drop` after a panic, never use `catch_unwind` on a production path, and a child process must not outlive its parent (`crates/browser/src/child/mod.rs` exits on channel EOF).
- A panic must leave a record. The panic hook writes synchronously to the log file before the abort (`crates/logging/src/crash.rs`); the async log queue is lost on abort. Keep crash-path code panic-free.

## Method

- Measure the ceiling before designing. Prove what a change can win (stub the code out, read `.text`, count call sites) before writing it.
- Record flat experiments. Negative results are results, so nobody retries them: outlining generated groups, `#[inline(never)]` on conversion helpers, link-time ICF, and dictionary dedup all measured flat or negligible.
- Data over code. Generated glue becomes tables plus shared funnels: monomorphize per IDL type, never per member.
- Parity gates every refactor. WPT before/after must match exactly (same pass/unexp/timeout/crash counts), and any release-binary change gets a WPT smoke run via `--binary`.
- Delete stale config when its reason expires. Workarounds (bfd linker flags, an extra nightly dev shell, the post-link script, an unused wasm target) are removed with the reason recorded in the commit, not left to rot.
- Forks are for correctness, not micro-wins. A fork change needs a spec or crash reason recorded in `docs/upstream.md`; compile or size tricks stay upstream.

## Pointers

- **Web API bindings & IDL**: Read `docs/bindings.md` for JS/Rust ownership, WebIDL contracts, and platform objects.
- **Engine behavior**: Read `docs/engines.md` to search and fetch Chromium/Firefox sources without cloning.
- **Engine forks & submodules**: Read `third_party/README.md` for submodule maintenance, fork branches, and update workflows.
- **WPT & conformance harness**: Read `tools/wpt/README.md` for test runners, scoring, and failure diffing. Check `.github/workflows/wpt.yml` runs (`gh run list --workflow=wpt.yml`) against `docs/progress.md` for regressions.
- **Upstream gaps, priorities, snapshot**: See `docs/upstream.md`, `docs/priorities.md`, `docs/progress.md`.
- **Project notes**: `~/Documents/obsidian/everything/projects/tinybrowser/`
