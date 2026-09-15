# tinybrowser

We are building the smallest and lightest headless browser for AI agents.
The target binary size is under 10MB stripped on x86_64. Measure binary size at milestones.

## Testing and conformance

- Cargo tests cover tinybrowser-specific behavior only.
- Web-platform conformance is WPT's job(`tools/wpt/run`).

Do not test spec conformance in cargo tests. Never add, keep, or "fix" a cargo test that asserts web-platform behavior or duplicates a WPT case. If a spec regression would only be caught by a cargo test, the missing WPT run is the bug.

## Working rules

- Never write `unsafe`: no `unsafe {}`, `unsafe fn`, `unsafe trait`, `unsafe impl`, or `#[allow(unsafe_code)]`. The workspace already `deny`s `unsafe_code`. If a dependency trait is unsafe (rquickjs `JsLifetime`), implement it only with that crate's derive. Do not hand-write the impl. If the derive cannot be used, the design is wrong: change the types so they hold no JS lifetimes, or stop using the API that demanded `unsafe`.
- Never silence the compiler or a lint to make an error go away. When a check fires, find the design flaw it points at and fix that. An `#[allow]`/`unwrap`-style escape needs a written justification at the same spot and is a last resort.
- Breaking changes and full rewrites are always fine. When code fights you, assume it is wrong: zoom out, fix the design, don't patch around it.
- Web-platform behavior comes from the WHATWG specs, not from our tests or guesses. Start every implementation and review of a conformance claim from the governing spec. Cite the spec at the implementation site with an anchor link, for example `dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity`. Follow the spec's exact algorithm order, even when our tests disagree. Fix the test, not the spec.
- For how a browser actually behaves, read a shipped engine implementation. Start with Chromium, then read Firefox when Chromium does not cover the case. Do not clone these repositories. When the spec and a shipped browser disagree, state the disagreement in a code comment and follow the browser that matches the spec algorithm.

## Reference

- Read [docs/CONTEXT.md](docs/CONTEXT.md) for domain terms.
- Read [docs/adrs/](docs/adrs/) for architectural decisions.
- Project notes live in the Obsidian vault at `~/Documents/obsidian/everything/projects/tinybrowser/`.
