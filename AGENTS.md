# tinybrowser

We build a headless browser for AI agents as one executable. There is no separately shipped or versioned helper program such as chromedriver or Node. The same tinybrowser executable may self-spawn a detached profile daemon now and renderer workers in a later security phase. Dynamic linking for standard system libraries like libc and OpenSSL is fine. The target binary size is under 5MB stripped on x86_64. Measure binary size at milestones.

## Working Rules

- Avoid `unsafe`. Use `unsafe` only after proving a safe design is impossible. Document every `unsafe` block with a `// SAFETY:` comment that explains the invariant.
- Fix compiler errors and lints at the root cause. Do not silence checks with `#[allow]` or unwrap calls. If an escape is unavoidable, explain the reason in a comment.
- Redesign code that resists changes instead of patching around flaws. Full rewrites are welcome.
- Keep dependencies small. Do not add server frameworks or heavy async runtimes like full Tokio. See [docs/adrs/0007-engine-charter.md](docs/adrs/0007-engine-charter.md).
- Run network calls with `spawn_blocking`. Do not block the page thread.
- Follow WHATWG specifications for web platform behavior. Link to the exact spec section in comments next to the code.
- Check Firefox for real-world browser behavior. Inspect single files through [searchfox.org](https://searchfox.org) or `raw.githubusercontent.com`. Do not clone the repository.
- Run `git submodule update --init --recursive` before running tests in `crates/browser`.

## Reference

- Read [docs/CONTEXT.md](docs/CONTEXT.md) for domain terms.
- Read [docs/adrs/](docs/adrs/) for architectural decisions.
- Read [docs/researches/size-budget.md](docs/researches/size-budget.md) for size tracking.
