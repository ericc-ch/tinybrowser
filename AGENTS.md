# tinybrowser

We are building the smallest and lightest headless browser for AI agents.
The target binary size is under 10MB stripped on x86_64.

In `docs/progress.md`, replace the latest binary size, the latest total, and scored groups only.

## Testing and conformance

- Cargo tests cover tinybrowser-specific behavior only (`cargo test --workspace`).
- Web-platform conformance is WPT (`tools/wpt/run`, `score`, `retest`).
- Extra runners: Blink CDP (`tools/cdp/run`), Playwright (`tools/playwright/run`), test262 (`tools/intl/test262`).

Do not test spec conformance in cargo tests. Never add, keep, or "fix" a cargo test that asserts web-platform behavior or duplicates a WPT case. If a spec regression would only be caught by a cargo test, the missing WPT run is the bug.

## Checks

- clippy (`tools/ub lint`), `cargo test --workspace`.
- When adding or changing `unsafe`: `tools/ub miri` (pure-Rust) and `tools/ub valgrind` (FFI / renderer / net).

## Working rules

Never maintain handwritten `unsafe`: no `unsafe {}`, `unsafe fn`, `unsafe trait`, `unsafe impl`, or `#[allow(unsafe_code)]` in tinybrowser-owned code, unless:

- The runtime safety is proven (a `// SAFETY:` invariant a reviewer can check, tests that exercise it, and Miri or a sanitizer where the tooling can see the code)
- No safe alternative is as simple, clear, or fast.
  Unsafe code maintained by upstream dependencies is allowed, including code emitted into this workspace by an upstream derive or bindings generator such as rquickjs or wit-bindgen.
  Narrow `#[expect(...)]` attributes with a written reason may wrap an upstream generator invocation for lints in its emitted code; never place handwritten unsafe inside the same scope. Do not manually edit or locally fork generated unsafe code; doing so makes it ours.
  The workspace otherwise `deny`s `unsafe_code`.

- Never silence the compiler or a lint to make an error go away. When a check fires, find the design flaw it points at and fix that. An `#[allow]`/`unwrap`-style escape needs a written justification at the same spot and is a last resort.
- Breaking changes and full rewrites are always fine. When code fights you, assume it is wrong: zoom out, fix the design, don't patch around it.
- Web-platform behavior comes from the WHATWG specs, not from our tests or guesses. Start every implementation and review of a conformance claim from the governing spec. Cite the spec at the implementation site with an anchor link, for example `dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity`. Follow the spec's exact algorithm order, even when our tests disagree. Fix the test, not the spec.
- For how a browser actually behaves, read a shipped engine implementation. Start with Chromium, then read Firefox when Chromium does not cover the case. Do not clone these repositories. When the spec and a shipped browser disagree, state the disagreement in a code comment and follow the browser that matches the spec algorithm.
- Decisions live in commit messages.
- Do experimentations in `/tmp/`. Scratch builds, size probes, throwaway JS/Rust experiments, etc.

## Reference

- Project notes live in the Obsidian vault at `~/Documents/obsidian/everything/projects/tinybrowser/`.
