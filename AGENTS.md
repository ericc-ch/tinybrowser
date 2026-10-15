We are building the smallest, lightest, and fastest headless browser for AI agents.
Maximize web spec conformance per byte.

- WHATWG specs are ground truth. Start every implementation and review from the spec. Follow exact algorithm order and cite the spec URL at the implementation site.
- Heavily reference mature engines. Read Chromium/Firefox source code (see `docs/engines.md`). Never clone the repositories. When the spec and a shipped browser disagree, state the disagreement in a code comment and follow the browser that matches the spec algorithm.
- Breaking changes, churn, and rewrites are always welcome. When code fights you, zoom out and fix the architectural design. Do not work or patch around it.
- No lint silencing. Fix the underlying design flaw instead of suppressing compiler or lint diagnostics.
- Handwritten `unsafe` is prohibited unless runtime safety is proven (`// SAFETY:` invariant, Miri/valgrind checks via `tools/check`) and no safe alternative is as simple, clear, or fast.
- Let upstream bugs fail honestly. Never build tinybrowser workarounds for upstream engine bugs; record gaps in `docs/upstream.md`.
- Keep feedback loop fast. Cargo tests cover tinybrowser internals only (`cargo test --workspace`); never test web-platform spec conformance in cargo tests. Run the smallest slice that answers the question (single files or directories, never full suites). Never run long or whole-suite test runs without explicit approval.
- Commit messages should also record decisions.
- Throwaway experiments belong in `/tmp/`.

## Pointers

- **Web API bindings & IDL**: Read `docs/bindings.md` for JS/Rust ownership, WebIDL contracts, and platform objects.
- **Engine behavior**: Read `docs/engines.md` to search and fetch Chromium/Firefox sources without cloning.
- **Engine forks & submodules**: Read `third_party/README.md` for submodule maintenance, fork branches, and update workflows.
- **WPT & conformance harness**: Read `tools/wpt/README.md` for test runners, scoring, and failure diffing.
- **Upstream gaps, priorities, snapshot**: See `docs/upstream.md`, `docs/priorities.md`, `docs/progress.md`.
- **Project notes**: `~/Documents/obsidian/everything/projects/tinybrowser/`
