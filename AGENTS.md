We are building the smallest and fastest headless browser for AI agents.

Sub-5MB stripped x86_64 binary is the goal. Measure size at milestones.
Prefer the light path that stays fast for one page: our HTML job list, Tokio only as the waiter, not a web-server stack.
Single binary, no cheating, no sidecar processes.

## Working rules

- `unsafe` is forbidden by default. Reach for it only after a safe design is proven impossible; prove it by attempting it, not by assuming it can't exist.
- Every `unsafe` block carries a `// SAFETY:` comment spelling out the invariant that makes it sound (the linter already rejects missing ones).
- Never silence the compiler or a lint to make an error go away. When a check fires, find the design flaw it points at and fix that. An `#[allow]`/`unwrap`-style escape needs a written justification at the same spot and is a last resort.
- Breaking changes and full rewrites are always fine. When code fights you, assume it is wrong: zoom out, fix the design, don't patch around it.
- Web-platform behavior is defined by the WHATWG specs. When implementing or reviewing a conformance claim, cite the governing spec section right where it is implemented (anchor links, e.g. `dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity`).
- For engine ground truth on how browsers actually behave (read order, search URLs, single-file fetch, do not clone): [`wiki/researches/engine-source.md`](wiki/researches/engine-source.md).

## Environment

- `cargo test -p browser` needs the vendored html5lib suite; initialize it with
  `git submodule update --init --recursive`.

## Further Reading

- [`wiki/README.md`](wiki/README.md)
