# tinybrowser

We are building a headless browser for AI agents as one executable. There is no separately shipped or versioned helper program such as chromedriver or Node. The same tinybrowser executable may self-spawn a detached profile daemon and one renderer process per site instance ([ADR 0011](docs/adrs/0011-renderer-processes-per-site.md)); sandboxing renderers is a later security phase. Dynamic linking for standard system libraries like libc and OpenSSL is fine. The target binary size is under 10MB stripped on x86_64. Measure binary size at milestones.
Prefer the light path that stays fast for one page: our HTML job list, Tokio only as the waiter, not a web-server stack.

## Working rules

- Never write `unsafe`: no `unsafe {}`, `unsafe fn`, `unsafe trait`, `unsafe impl`, or `#[allow(unsafe_code)]`. The workspace already `deny`s `unsafe_code`. If a dependency trait is unsafe (rquickjs `JsLifetime`), implement it only with that crate's derive. Do not hand-write the impl. If the derive cannot be used, the design is wrong: change the types so they hold no JS lifetimes, or stop using the API that demanded `unsafe`.
- Never silence the compiler or a lint to make an error go away. When a check fires, find the design flaw it points at and fix that. An `#[allow]`/`unwrap`-style escape needs a written justification at the same spot and is a last resort.
- Breaking changes and full rewrites are always fine. When code fights you, assume it is wrong: zoom out, fix the design, don't patch around it.
- Web-platform behavior is defined by the WHATWG specs. When implementing or reviewing a conformance claim, cite the governing spec section right where it is implemented (anchor links, e.g. `dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity`).
- For engine ground truth on how browsers actually behave, read Firefox's implementation: fetch single files from `github.com/mozilla-firefox/firefox` (via `raw.githubusercontent.com`) or search [searchfox.org](https://searchfox.org). Never clone that repo, it is far too big.
- One OS thread owns a document (`Dom`, QuickJS) inside a renderer process; the host owns tabs, navigation, and the renderer registry ([ADR 0011](docs/adrs/0011-renderer-processes-per-site.md)). That thread runs HTML jobs from a queue we own (parse, script, timer, `fetch` callback). The renderer thread is a Tokio **current-thread** runtime with features `rt` and `time` only. Blocking network work runs on the browser-owned pool (16 workers, 256-job queue); renderers reach it through the value-only host seam, never a blocking `send()` on the renderer thread.
- Do not add tokio `full` or smol. axum/hyper are allowed only in the host protocol adapters and clap only in the CLI ([ADR 0012](docs/adrs/0012-host-protocol-and-cli-stack.md)); the renderer path takes none of them. Size numbers: [`docs/researches/size-budget.md`](docs/researches/size-budget.md). Crate and loop decisions: [`docs/adrs/0007-engine-charter.md`](docs/adrs/0007-engine-charter.md).

## Reference

- Read [docs/CONTEXT.md](docs/CONTEXT.md) for domain terms.
- Read [docs/adrs/](docs/adrs/) for architectural decisions.
- Read [docs/researches/size-budget.md](docs/researches/size-budget.md) for size tracking.
- Read [docs/researches/engine-source.md](docs/researches/engine-source.md) before fetching Firefox, Chromium, or WebKit files.
