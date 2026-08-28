# Autonomous run: engine charter implementation (2026-08-27)

Exit: workspace is `dom`/`net`/`browser`/`tinybrowser`; no `js` crate; root depends on `browser` only; template contents live on `Dom` and html5lib dump reads `Dom`; `Page` holds `Agent`, runs HTML jobs on Tokio current-thread `rt`+`time`, and performs HTTP via `spawn_blocking`. html5lib 3549 with 10 pinned upstream dumps. Page tests include three host contracts (eval stringify, fetch `text()`, `ScriptFailed`) that were red until the host matched them.

| iter | what | why | evidence | result |
|---|---|---|---|---|
| 0 | crate graph already in tree | prior turn | cargo tree | kept |
| 1 | `Dom` template map | ticket 03 | `template_contents_live_on_dom_*` | green |
| 2 | sink + html5lib dump on `Dom` | dump must not use `Parsed` | parse + html5lib | green |
| 3 | `Page` loop + `spawn_blocking` | tickets 02/04 | `host_timer_runs_while_fetch_*` | spawn needed runtime; queued fetches |
| 4 | tokio `macros` | `select!` | clippy/test | green; not `full` |
| 5 | size | charter | 294944 byte stub CLI | LTO drops unused Page |

Predicate: met. Not committed (user did not ask).
