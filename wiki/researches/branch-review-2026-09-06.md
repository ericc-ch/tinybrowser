# Branch review, 2026-09-06

The fetched inventory had four non-main remote branches, two matching local branches, and one open PR. The working tree was clean. None of the branch tips conflicted with main.

| Branch | Initial tip | Local | Merge assessment and review |
| --- | --- | --- | --- |
| `cursor/net-types-send-loopback` | `b4f055c` | yes | Already included in main through PR #1. Reviewed the current transport, cookie, redirect, and WebSocket paths. Fixed IPv6 dialing, proxy debug redaction, and a flaky loopback fixture in the integration changes. |
| `cursor/merge-pr-1-2004` | `b1e9f4c` | no | Already included in main. Merge commit for the preceding branch; no separate feature to merge. Its preserved environment instructions remain relevant. |
| `cursor/remove-stale-wiki-works-2004` | `fce9e67` | no | Already included in main. Documentation cleanup, with lasting decisions retained in ADRs. No additional code change. |
| `feat/page-thread-js-host` | `7a11b99` | yes | PR #2, based directly on main. Technically a fast-forward before review. Reviewed parser, DOM, JS host, page loop, dependency changes, tests, and documentation. Repairs below are included before merging. |

## Findings repaired

| Severity | Kind | Finding and resolution |
| --- | --- | --- |
| must-fix | bug | Template associations allowed ownership cycles, including insertion of a template into its own contents. Mutation checks now include template hosts. |
| must-fix | bug | Deep cloning used Rust recursion on page-controlled trees. Cloning now uses an explicit work list. |
| should-fix | bug | Shallow template clones copied the contents subtree. They now receive empty contents; deep clones copy descendants. |
| should-fix | bug | A thrown script skipped the microtask checkpoint. Cleanup now runs on both success and failure, and the page adopts queued work even after an eval error. |
| should-fix | bug | Rejection handlers for invalid fetch URLs could queue work that the page left pending when it returned or parked. The pump checks pending host work before waiting. |
| should-fix | bug | Fired timers retained their callback closures in JS. Consumed slots are now deleted. |
| should-fix | bug | `load_html` left a queued navigation able to overwrite the replacement document. It now invalidates and drops queued navigation work. |
| should-fix | bug | Language selection gave unnamespaced `lang` precedence over `xml:lang`. The order and the affected test now match HTML; unnamespaced `lang` is limited to HTML/SVG elements. |
| should-fix | bug | Option cloning ignored disabled selectedcontent ancestry. The lookup now checks that ancestry and walks iteratively. Selection no longer allocates a parallel boolean array. |
| should-fix | design | Tokio enabled `macros` despite the repository's `rt` + `time` requirement. The waiter uses safe standard future polling; unused QuickJS macros were also removed. |
| should-fix | bug | URL-form IPv6 brackets reached the socket resolver. They are removed at the TCP boundary. An IPv6 loopback request verifies the fix. |
| must-fix | security | Derived debug output exposed proxy credentials from agent configuration. AgentBuilder and the connector now expose only proxy presence and nonsecret configuration. |
| should-fix | test | The method-casing fixture closed sockets without advertising `Connection: close`, racing connection reuse. The failure reproduced before the fix; 30 consecutive runs passed afterward. |
| should-fix | clarity | PR #2 removed active runtime/build rules and left wiki links to deleted work notes. The rules are restored and references now point at the durable engine ADR. |
| should-fix | test | The size checkpoint measured only a CLI stub. A reproducible page probe now links parsing, navigation, JS eval, timers, and the page loop. |

## Validation

All commands ran through the Nix devshell, using rustc/cargo 1.98.0.

- `cargo test --workspace`: passed. The live-network probes and accepted-dump writer remain explicitly ignored.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- html5lib: 3,549 runs; 10 matched the pinned, documented upstream divergences. No new divergence was accepted.
- `cargo build --release --example page_probe --bin tinybrowser`: passed.
- `cargo run --release --example page_probe`: printed `42`.
- Stripped x86_64 CLI stub: 294,944 bytes. Page probe: 2,831,904 bytes. Both use the committed release profile. The probe links native TLS dynamically through the Nix environment.

## Scope and remaining limits

This merges a page-engine foundation. The CLI commands are still stubs. Loading HTML does not execute its script elements, and the JS host is a small subset of browser APIs: it has no DOM bindings beyond cookies, full Fetch/CORS implementation, timer cancellation, or script execution budget. The size probe is not a finished browser-size claim. Existing transport limitations, including the origin-form HTTP forward-proxy path and non-deadline-bounded system DNS resolution, remain outside these repairs.

PR #1 was already merged. PR #2 is the only PR requiring a merge. Branch cleanup removes redundant refs after their tips are reachable from main; merged PR records remain in GitHub history.

## Governing sources

- [DOM pre-insert validity](https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity) and [host-including ancestry](https://dom.spec.whatwg.org/#concept-tree-host-including-inclusive-ancestor).
- [HTML template cloning](https://html.spec.whatwg.org/multipage/scripting.html#the-template-element) and [Firefox template host association](https://github.com/mozilla-firefox/firefox/blob/main/dom/html/HTMLTemplateElement.cpp).
- [HTML script cleanup](https://html.spec.whatwg.org/multipage/webappapis.html#clean-up-after-running-script).
- [HTML language determination](https://html.spec.whatwg.org/multipage/dom.html#language).
- [HTML selectedcontent](https://html.spec.whatwg.org/multipage/form-elements.html#the-selectedcontent-element).
