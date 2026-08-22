We are building the smallest headless browser for AI agents.
Single binary, no cheating, no sidecar processes.
Sub-5MB stripped x86_64 binary is the goal. Measure size at milestones.

## Working rules

- `unsafe` is forbidden by default. Reach for it only after a safe design is proven impossible; prove it by attempting it, not by assuming it can't exist.
- Every `unsafe` block carries a `// SAFETY:` comment spelling out the invariant that makes it sound (the linter already rejects missing ones).
- Never silence the compiler or a lint to make an error go away. When a check fires, find the design flaw it points at and fix that. An `#[allow]`/`unwrap`-style escape needs a written justification at the same spot and is a last resort.
- Breaking changes and full rewrites are always fine. When code fights you, assume it is wrong: zoom out, fix the design, don't patch around it.

## Layout

Workspace crates `dom`, `net`, `js`, `browser`; root package `tinybrowser` holds the embedder lib and `serve`/`fetch` bins. Deps go downward only; `browser` is the only fan-in point. CDP lands later depending on `browser` alone — see `docs/adr/0001-workspace-crates-with-enforced-edges.md`.

## Further Reading

- `docs/size-budget.md` — binary size measurements and decisions
- `docs/webidl.md` — DOM API surface verification strategy
