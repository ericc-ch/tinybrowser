# Session: engine charter (2026-08-27)

State: Planning closed ([spec.md](./spec.md), [ADR 0007](../../adrs/0007-engine-charter.md)). Wiki/comments match; **no engine code**. Dirty `main` vs `9131250` / `origin/main`. Decision tickets 01–07 are answers, not implementation slices.

Done:
- [map.md](./map.md) complete; [spec.md](./spec.md)
- ADR 0007 live; 0001 history; 0002/0006/CONTEXT/`AGENTS.md` aligned
- Size probes recorded in [size-budget.md](../../researches/size-budget.md) (tokio `rt`+`time` +66 KB tuned)

In flight:
- Uncommitted docs/rustdoc/`state.rs`/`agent.rs` comments
- Cargo still has empty `crates/js`; `browser` and root still depend on `js`/`dom`/`net` (charter: root → `browser` only)

Next:
1. Commit the docs pass
2. Optional `write-tickets` from the spec; or `do-work` in this order: drop unused `js` dep + root fan-in → template map onto `Dom` (html5lib dump via `Dom`) → Tokio current-thread `rt`+`time` on the page thread → `spawn_blocking` + `Agent` when navigation exists
3. QuickJS/host APIs after a loop exists. Stealth later later.

Decisions made: see spec. Do not reopen arena, html5ever, selectors-in-dom, wreq, `HttpTransport`, tokio `full`/axum/hyper.

Gotchas:
- `nix develop --command cargo …` (rustc 1.98). `git submodule update --init` before `cargo test -p browser`
- `unsafe_code = deny` in *this* repo; rquickjs may use unsafe internally
- Put Tokio on `browser`, not `net`. Never `Agent::send()` on the page thread
- QuickJS is `eval` / `execute_pending_job` / `call(fn)`. Host timers are our Vec. Throwaways `/tmp/qjs-host-demo`, `/tmp/js-engine-loop.html` may be gone
- html5lib: 3165 full-document green; 192 fragment cases still skipped until fragment parse
- Measure stripped size when Tokio or template-on-Dom lands
- Firefox: fetch single files / searchfox; never clone

Suggested skills: do-work (playbook: feature), write-tickets, tdd, review
