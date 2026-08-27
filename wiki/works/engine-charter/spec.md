# Engine charter

Problem: The crate rules and leftover net-era docs over-policed imports and parked DOM/language/persistence on the wrong layer. The holes that actually block a page (job loop, template state, who holds HTTP) were unwritten or wrong. A 5 MB headless agent browser still has to run script without freezing, without a web-server stack.

Solution: Three deep crates, one compile-error law for later CDP, an HTML job list on a Tokio current-thread waiter, `Dom` owning template contents, `browser` holding `net::Agent` and `parse_html`. Stealth and QuickJS stay later. Wiki/ADR slogans get rewritten to match.

User stories:

1. As an embedder, I depend on `tinybrowser` → `browser` only, so I do not import `dom`/`net` by accident.
2. As a page, I run `setTimeout` and `fetch` without freezing the JS turn; blocking HTTP waits on `spawn_blocking`.
3. As `template.content` / html5lib dump, I read fragment contents from `Dom`, not a parser leftover map.
4. As page script, `document.cookie` talks to the `Agent` jar; language and `Content-Language` are document state.
5. As a later stealth effort, I swap `net` internals without changing `browser`’s types.

Implementation decisions:

- Crates: `dom`, `net`, `browser`. No `js` crate until QuickJS has a small public surface. Root depends on `browser` only. Future `cdp` depends on `browser` alone. Tests may `cargo test -p` a leaf crate.
- Page thread: Tokio current-thread, features `rt` + `time` only (~+66 KB tuned). HTML tasks/microtasks are our queue. `Agent::send` / `upgrade` only via `spawn_blocking`. No tokio `full`, smol, axum, hyper.
- `net` stays blocking ureq + native-tls. Public types stay ours. Stealth (Chrome TLS/h2) is a real milestone, later later.
- `browser` holds `net::Agent`. No `HttpTransport` trait. Relative URL / `<base>` resolve in `browser`.
- `parse_html` and the TreeSink stay in `browser`. Template element → contents fragment lives on `Dom`. Sink integration-point flags stay parse-only.
- Cookie jar stays on `Agent` (`cookies_for` / `set_cookie` as non-HTTP jar API). `document.cookie` is a page host function. Persistence is in-memory until a profile/embedder saves; not CDP.
- `:lang()` stays in `dom`. `xml:lang` is an attribute there. `Content-Language` is stored on the document by `browser` after navigation.

Testing decisions:

- html5lib dump uses `Dom` for template contents (no `Parsed` side map).
- Host-loop behavior is proven by a throwaway QuickJS demo pattern: eval, `execute_pending_job`, then fire saved timers; product tests come when JS is wired.
- Cookie tests stay on `Agent` as jar tests. `document.cookie` tests appear with the page host.
- Size: re-measure when Tokio `rt`+`time` lands on the page thread; forbid `full`.

Out of scope:

- Implementing QuickJS, navigation, CDP, stealth/btls, or axum/hyper
- Reopening the arena, html5ever vs html5gum, selectors-in-dom, or wreq
- Rewriting `net` as async

Notes:

- Map: [map.md](./map.md)
- Keep: [ADR 0002](../../adrs/0002-dom-layer-architecture.md) arena; [ADR 0006](../../adrs/0006-net-transport.md) v1 transport; [size-budget.md](../../researches/size-budget.md) async probes
- Follow-on wiki pass done: [ADR 0007](../../adrs/0007-engine-charter.md), ADR 0001 history-only, ADR 0002 language/template, CONTEXT terms.
