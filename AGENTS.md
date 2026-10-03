# tinybrowser

We are building the smallest and lightest headless browser for AI agents.
The target binary size is under 10MB stripped on x86_64.

In `docs/progress.md`, replace the latest binary size, the latest total, and scored groups only.

## Testing and conformance

- Cargo tests cover tinybrowser-specific behavior only (`cargo test --workspace`).
- Web-platform conformance is WPT (`tools/wpt/run`, `tools/wpt/run --score`, `retest`).
- Extra runners: Blink CDP (`tools/cdp-tests/run`), Playwright (`tools/playwright/run`), test262 (`tools/intl/test262`).
- Keep the WPT feedback loop fast: run the smallest slice that answers the question (single files or dirs, never suites); rerun only failures with `retest`.
- No suite or slice runs without being asked. Anything longer than minutes needs explicit approval.

Do not test spec conformance in cargo tests. Never add, keep, or "fix" a cargo test that asserts web-platform behavior or duplicates a WPT case. If a spec regression would only be caught by a cargo test, the missing WPT run is the bug.

## Checks

- clippy and embedded JS (`tools/check`), `cargo test --workspace`.
- Unless already inside the dev shell, run direct Cargo commands and runners that build the browser through `nix develop --command`; the host shell may not expose `pkg-config` or OpenSSL. `tools/check` already enters the Nix shell for clippy.
- When adding or changing `unsafe`: `tools/check miri` (pure-Rust) and `tools/check valgrind` (FFI / renderer / net).

## Dependencies

- Engine forks, one maintained branch each (`master` is the line). Fixes stay
  downstream; do not submit an upstream PR.
  - QuickJS-NG fork `github.com/ericc-ch/quickjs`. Checked out as the nested
    `sys/quickjs` submodule inside the rquickjs checkout below.
  - rquickjs fork `github.com/ericc-ch/rquickjs`. Checked out as the
    `third_party/rquickjs` submodule (recursive) and wired in via
    `[patch.crates-io]` path entries. It is its own workspace, excluded from
    the tinybrowser workspace, so its lint config stays separate.
  - Pull upstream inside the submodule, push the fork branch there, then bump
    the submodule pointer in tinybrowser. Push before pinning: a pointer to
    an unpushed commit breaks every other checkout at init time (`not our
    ref`). Confirm the commit is on the remote with
    `git ls-remote origin <sha>` inside the submodule before recording it.
    Fresh clones and worktrees need
    `git submodule update --init --recursive`.

- Use pinned upstream WebIDL extracts as the interface contract. Implementations
  follow standardized names and generated contracts. Do not maintain explicit
  mapping tables, per-member overrides, or modified spec declarations.
  Implementation follows IDL, never the reverse: do not edit IDL to match our code.

For Web API implementation or binding changes, read
`docs/bindings.md` for JS/Rust ownership, IDL inputs, and coverage rules.

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
