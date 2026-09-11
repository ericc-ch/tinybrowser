# tinybrowser

We are building a headless browser for AI agents as one executable. There is no separately shipped or versioned helper program such as chromedriver or Node. The same tinybrowser executable may self-spawn a detached browser process (`--daemon`) and one renderer process per site instance ([ADR 0011](docs/adrs/0011-renderer-processes-per-site.md)); sandboxing renderers is a later security phase. Dynamic linking for standard system libraries like libc and OpenSSL is fine. The target binary size is under 10MB stripped on x86_64. Measure binary size at milestones.
Prefer the light path that stays fast for one document: our task list, Tokio only as the waiter, not a web-server stack.

## Testing and conformance

Cargo tests and WPT answer different questions. Keep them that way.

- **Cargo tests cover tinybrowser-specific behavior only**: our product surface, protocol and transport, engine lifecycle, resource budgets, and deterministic invariants we own. They are not evidence of browser or web-platform behavior.
- **Web-platform conformance is WPT's job** (`tools/wpt/run`, e.g. `./tools/wpt/run dom/nodes/MutationObserver-*.html`). Spec behavior changes need a WPT result, not a new cargo test.
- **Do not test spec conformance in cargo tests.** Never add, keep, or "fix" a cargo test that asserts web-platform behavior or duplicates a WPT case. If a spec regression would only be caught by a cargo test, the missing WPT run is the bug.
- A cargo test may still drive a scenario that has a web-facing shape (e.g. engine and document API tests), but assert only tinybrowser-owned behavior: errors we define, task ordering, frame hosting, budget enforcement, protocol encoding.

## Working rules

- Never write `unsafe`: no `unsafe {}`, `unsafe fn`, `unsafe trait`, `unsafe impl`, or `#[allow(unsafe_code)]`. The workspace already `deny`s `unsafe_code`. If a dependency trait is unsafe (rquickjs `JsLifetime`), implement it only with that crate's derive. Do not hand-write the impl. If the derive cannot be used, the design is wrong: change the types so they hold no JS lifetimes, or stop using the API that demanded `unsafe`.
- Never silence the compiler or a lint to make an error go away. When a check fires, find the design flaw it points at and fix that. An `#[allow]`/`unwrap`-style escape needs a written justification at the same spot and is a last resort.
- Breaking changes and full rewrites are always fine. When code fights you, assume it is wrong: zoom out, fix the design, don't patch around it.
- Web-platform behavior is defined by the WHATWG specs, not by our tests or guesses. Every implementation and review of a conformance claim starts from the governing spec and cites it at the implementation site (anchor links, e.g. `dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity`). Follow the spec section, including the spec's exact algorithm order, even when our current tests disagree; fix the test, not the spec.
- For engine ground truth on how browsers actually behave, read the most mature implementation available: Firefox (`github.com/mozilla-firefox/firefox` single files via `raw.githubusercontent.com`, or [searchfox.org](https://searchfox.org)), then Chromium or WebKit when Firefox does not cover it. Never clone these repos. When spec and a shipped browser disagree, say so explicitly in the code comment and follow the browser that matches the spec's current algorithm.
- One OS thread owns a document (`Dom`, QuickJS) inside a renderer process; the browser process owns tabs, navigation, and the renderer registry ([ADR 0011](docs/adrs/0011-renderer-processes-per-site.md)). That thread runs tasks from a task queue we own (parse, script, timer, `fetch` callback). The renderer thread is a Tokio **current-thread** runtime with features `rt` and `time` only. Blocking network work runs on the browser-owned pool (16 workers, 256-task queue); renderers reach it through the value-only browser seam, never a blocking `send()` on the renderer thread.
- Do not add tokio `full` or smol. axum/hyper are allowed only in the browser-process protocol adapters and clap only in the CLI ([ADR 0012](docs/adrs/0012-host-protocol-and-cli-stack.md)); the renderer path takes none of them. Size numbers: [`docs/researches/size-budget.md`](docs/researches/size-budget.md). Crate and loop decisions: [`docs/adrs/0007-engine-charter.md`](docs/adrs/0007-engine-charter.md).

## Reference

- Read [docs/CONTEXT.md](docs/CONTEXT.md) for domain terms.
- Read [docs/adrs/](docs/adrs/) for architectural decisions.
- Read [docs/researches/size-budget.md](docs/researches/size-budget.md) for size tracking.
- Read [docs/researches/engine-source.md](docs/researches/engine-source.md) before fetching Firefox, Chromium, or WebKit files.
