# tinybrowser

We are building the smallest and lightest headless browser for AI agents.
The target binary size is under 10MB stripped on x86_64.

In `docs/progress.md`, replace the latest binary size, the latest total, and scored groups only.

## Testing and conformance

- Cargo tests cover tinybrowser-specific behavior only (`cargo test --workspace`).
- Web-platform conformance is WPT (`tools/wpt/run`, `tools/wpt/run --score`, `retest`).
- Extra runners: Blink CDP (`tools/cdp-tests/run`), Playwright (`tools/playwright/run`), test262 (`tools/intl/test262`).

Do not test spec conformance in cargo tests. Never add, keep, or "fix" a cargo test that asserts web-platform behavior or duplicates a WPT case. If a spec regression would only be caught by a cargo test, the missing WPT run is the bug.

### Visual rendering work

- Capture tinybrowser and Chromium with the same viewport and a fresh logged-out profile. Keep baseline images and browser-driving scripts in `/tmp/`.
- Record the User-Agent with each live-site baseline. Google serves legacy markup when the request has no browser User-Agent, while Chromium receives the modern page.
- Keep the default HTTP, JavaScript `navigator`, and CDP identities aligned when emulating Chrome. TLS fingerprinting is separate work.
- Record `prefers-color-scheme` with live-site baselines. Google changes its logo, canvas, controls, and footer together when Chromium reports dark mode.
- The render tree has a synthetic box above the document element. That box models the initial containing block. Give it the viewport as a definite size before resolving root percentage sizes.
- Paint the propagated `html` or `body` background on the canvas. Stretching the root box after layout does not implement canvas background propagation.
- A reftest can pass when both the test and reference omit the same unsupported feature. For image work, use a reference that paints with CSS instead of another image.
- Format touched Rust files directly. A workspace-wide format check can report unrelated formatter drift in untouched files.

## Checks

- clippy and embedded JS (`tools/check`), `cargo test --workspace`.
- Unless already inside the dev shell, run direct Cargo commands and runners that build the browser through `nix develop --command`; the host shell may not expose `pkg-config` or OpenSSL. `tools/check` already enters the Nix shell for clippy.
- When adding or changing `unsafe`: `tools/check miri` (pure-Rust) and `tools/check valgrind` (FFI / renderer / net).

## Working rules

Never maintain handwritten `unsafe`: no `unsafe {}`, `unsafe fn`, `unsafe trait`, `unsafe impl`, or `#[allow(unsafe_code)]` in tinybrowser-owned code, unless:

- The runtime safety is proven (a `// SAFETY:` invariant a reviewer can check, tests that exercise it, and Miri or a sanitizer where the tooling can see the code)
- No safe alternative is as simple, clear, or fast.
  Unsafe code maintained by upstream dependencies is allowed, including code emitted into this workspace by an upstream derive or bindings generator such as rquickjs or wit-bindgen.
  Narrow `#[expect(...)]` attributes with a written reason may wrap an upstream generator invocation for lints in its emitted code; never place handwritten unsafe inside the same scope. Do not manually edit or locally fork generated unsafe code; doing so makes it ours.
  The workspace otherwise `deny`s `unsafe_code`.

- Never silence the compiler or a lint to make an error go away. When a check fires, find the design flaw it points at and fix that. An `#[allow]`/`unwrap`-style escape needs a written justification at the same spot and is a last resort.
- No need to care about breaking changes, full rewrites, churns, etc. they are always fine. When code fights you, assume it is wrong: zoom out, fix the design, don't patch around it.
- Web-platform behavior comes from the WHATWG specs, not from our tests or guesses. Start every implementation and review of a conformance claim from the governing spec. Cite the spec at the implementation site with an anchor link, for example `dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity`. Follow the spec's exact algorithm order, even when our tests disagree. Fix the test, not the spec.
- For how a browser actually behaves, read a shipped engine implementation. Start with Chromium, then read Firefox when Chromium does not cover the case. Do not clone these repositories. When the spec and a shipped browser disagree, state the disagreement in a code comment and follow the browser that matches the spec algorithm.
- Decisions live in commit messages.
- Do experimentations in `/tmp/`. Scratch builds, size probes, throwaway JS/Rust experiments, etc.

## Reference

- Project notes live in the Obsidian vault at `~/Documents/obsidian/everything/projects/tinybrowser/`.
