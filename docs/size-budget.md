# Size Budget

Goal: **sub-5MB stripped x86_64 binary** (AGENTS.md Vision). Measured 2026-08-21, rustc 1.98.0, Linux.

Reproduce with `sizeprobe/` in this repo — each dependency is really exercised (tokenize, dial, JS eval), not just imported. Marginal = binary delta vs an empty-`main` build of the same profile.

## Measured marginal costs

| Component                                        | default `release` | tuned¹  |
| ------------------------------------------------ | ----------------- | ------- |
| baseline (empty main)                            | 345 KB            | 290 KB  |
| html5gum 0.8 (tokenizer only)                    | +388 KB           | +272 KB |
| **html5ever 0.39 + tree builder** (rcdom sink)   | +941 KB           | +840 KB |
| selectors 0.26 + cssparser (on top of html5ever) | +115 KB           | +75 KB  |
| ureq 3 + native-tls (dyn libssl.so.3)            | +753 KB           | +482 KB |
| rquickjs 0.12 (quickjs-ng, eval + limits)        | +1247 KB          | +776 KB |

¹ `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`, stripped

## Stack totals (tuned profile)

| Stack                                                  | Total       | Headroom to 5 MB |
| ------------------------------------------------------ | ----------- | ---------------- |
| ureq(native-tls) + quickjs-ng + **html5ever** ← chosen | **2.26 MB** | ~2.74 MB         |
| same but html5gum instead                              | 1.73 MB     | ~3.27 MB         |
| any of the above on default `release`                  | 2.7–3.3 MB  | —                |

## Decisions

- **html5ever over html5gum** (+~570 KB): buys the complete HTML5 tree-construction algorithm (insertion modes, foster parenting, adoption agency). html5gum is tokenizer-only; hand-rolling tree construction is weeks of fiddly spec work. Maturity wins under a 5MB budget.
- **Tuned profile from day one**: default release costs +400–850KB for nothing. The flags are set once in the root Cargo.toml.
- **native-tls, dynamically linked**: TLS lives in system `libssl.so.3`; we ship only glue (~482 KB tuned). Static rustls would add ~+2.0 MB — rejected. Consequence: target machine needs OpenSSL 3 installed (near-universal on Linux).
- **panic = unwind kept**: abort saves only ~39 KB but kills `catch_unwind`, which every JS-exposed op needs so a Rust panic degrades to a JS error instead of unwinding through QuickJS's C frame.
- **selectors later is cheap**: querySelector support costs only ~75 KB when we get to it.
- **Old servo stack (html5ever + selectors + cssparser as the _core_) was never the problem** — the old repo's total was bloat elsewhere. The parser swap alone does not hit 5MB; discipline at every milestone does.

## Watchlist (what can still blow the budget)

- DOM→JS binding glue: hundreds of rquickjs classes add up; keep dispatch tables data-driven.
- CDP server: tokio-tungstenite-style async stack is expensive; prefer a lean HTTP+WebSocket impl on `std::net`.
- A11y walker (accname computation, role mapping): budget ~100–200 KB, fine, but measure.
- Re-measure marginals at every milestone; regressions must justify themselves in bytes.
