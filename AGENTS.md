We are building the smallest headless browser for AI agents.
Single binary, no cheating, no sidecar processes.
Sub-5MB stripped x86_64 binary is the goal. Measure size at milestones.
Prefer the light path that stays fast for one page: our HTML job list, Tokio only as the waiter, not a web-server stack.

## Working rules

- `unsafe` is forbidden by default. Reach for it only after a safe design is proven impossible; prove it by attempting it, not by assuming it can't exist.
- Every `unsafe` block carries a `// SAFETY:` comment spelling out the invariant that makes it sound (the linter already rejects missing ones).
- Never silence the compiler or a lint to make an error go away. When a check fires, find the design flaw it points at and fix that. An `#[allow]`/`unwrap`-style escape needs a written justification at the same spot and is a last resort.
- Breaking changes and full rewrites are always fine. When code fights you, assume it is wrong: zoom out, fix the design, don't patch around it.
- Web-platform behavior is defined by the WHATWG specs. When implementing or reviewing a conformance claim, cite the governing spec section right where it is implemented (anchor links, e.g. `dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity`).
- For engine ground truth on how browsers actually behave, read implementations in this order: spec first, then Firefox (Gecko), then Chromium (Blink) if Gecko is silent or they disagree, then WebKit if those two still disagree. Search and fetch single files; never clone these repos, they are far too big. URLs and tree paths: [`wiki/researches/engine-source.md`](wiki/researches/engine-source.md).
  - Firefox: [searchfox.org/mozilla-central](https://searchfox.org/mozilla-central/source/), fetch `https://raw.githubusercontent.com/mozilla-firefox/firefox/<rev>/path`
  - Chromium: [source.chromium.org](https://source.chromium.org/chromium/chromium/src), fetch `https://raw.githubusercontent.com/chromium/chromium/<rev>/path` (page model is `third_party/blink/renderer/`)
  - WebKit: [searchfox.org/wubkat](https://searchfox.org/wubkat/source/), fetch `https://raw.githubusercontent.com/WebKit/WebKit/<rev>/path` (page model is `Source/WebCore/`)
- One OS thread owns a page (`Dom`, later JS). That thread runs HTML jobs from a queue we own (parse, script, timer, `fetch` callback). The thread is a Tokio **current-thread** runtime with features `rt` and `time` only. Waiting on the network is `spawn_blocking` around `Agent::send()`, never a blocking `send()` on the page thread.
- Do not add tokio `full`, smol, axum, or hyper. Those are a fat scheduler or HTTP *server* stacks. We are not a website. CDP, when it exists, is a thin socket on `std::net` unless a milestone proves otherwise. Size numbers: [`wiki/researches/size-budget.md`](wiki/researches/size-budget.md). Crate and loop decisions: [`wiki/works/engine-charter/map.md`](wiki/works/engine-charter/map.md).

## Environment

- The pinned toolchain (rustc/cargo 1.98, rustfmt, clippy, rust-src) and the OpenSSL that
  `native-tls` links against come from the nix flake devshell (`flake.nix`,
  `rust-toolchain.toml`). Run cargo inside it: `nix develop --command cargo …`
  (or `direnv allow` once, since `.envrc` runs `use flake`). The repo edition is
  2024, so the system's stock toolchain may be too old outside the devshell.
- `cargo test -p browser` needs the vendored html5lib suite; initialize it with
  `git submodule update --init --recursive`.

## Further Reading

- [`wiki/README.md`](wiki/README.md) — terms, ADRs, size/testing notes, engine source URLs
