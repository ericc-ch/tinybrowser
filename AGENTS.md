We are building the smallest headless browser for AI agents.
Single binary, no cheating, no sidecar processes.
Sub-5MB stripped x86_64 binary is the goal. Measure size at milestones.

## Working rules

- `unsafe` is forbidden by default. Reach for it only after a safe design is proven impossible; prove it by attempting it, not by assuming it can't exist.
- Every `unsafe` block carries a `// SAFETY:` comment spelling out the invariant that makes it sound (the linter already rejects missing ones).
- Never silence the compiler or a lint to make an error go away. When a check fires, find the design flaw it points at and fix that. An `#[allow]`/`unwrap`-style escape needs a written justification at the same spot and is a last resort.
- Breaking changes and full rewrites are always fine. When code fights you, assume it is wrong: zoom out, fix the design, don't patch around it.
- Web-platform behavior is defined by the WHATWG specs. When implementing or reviewing a conformance claim, cite the governing spec section right where it is implemented (anchor links, e.g. `dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity`).
- For engine ground truth on how browsers actually behave, read Firefox's implementation: fetch single files from `github.com/mozilla-firefox/firefox` (via `raw.githubusercontent.com`) or search [searchfox.org](https://searchfox.org). Never clone that repo, it is far too big.

## Environment

- The pinned toolchain (rustc/cargo 1.98, rustfmt, clippy) and the OpenSSL that
  `native-tls` links against come from the nix flake devshell (`flake.nix`,
  `rust-toolchain.toml`). Run cargo inside it: `nix develop --command cargo …`
  (or `direnv allow` once, since `.envrc` runs `use flake`). The repo edition is
  2024, so the system's stock toolchain may be too old outside the devshell.
- `cargo test -p browser` needs the vendored html5lib suite; initialize it with
  `git submodule update --init --recursive`.

## Further Reading

- [`wiki/README.md`](wiki/README.md) — terms, ADRs, size/testing notes, in-flight work
