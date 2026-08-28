# Session: engine charter (2026-08-27)

State: Charter implementation is in the worktree, **uncommitted**. Product slices for this effort are done. CDP and stealth still later.

Done (uncommitted vs `origin/main`):
- Crate graph, template-on-`Dom`, Tokio page waiter
- `parse_html_fragment`; html5lib 3549 green (html5ever selectedcontent still listed)
- `Page::goto`, `<base>` resolve, `Dom::document_language`
- Thin `QuickJS` host: `Page::eval`, `setTimeout`, `fetch` Promise `{ status, text() }`, `document.cookie`
- Three page contracts were red and are now the host: eval stringifies (`1+1` → `"2"`), JS fetch reads the body, timer/fetch callback throws push `PageEvent::ScriptFailed`. Stale JS fetch after `load_html` is ignored via `js_epoch`.

JS is a thin host. Promises live in QuickJS. `HtmlJob` is timer / dial-finished / dial-failed. Navigation and `load_html` drop the realm. Dials are `http`/`https` only. No private-IP SSRF policy yet.

Next:
1. Commit if wanted
2. CDP crate when needed
3. Stealth later later
4. Parser-driven `<script>` still not hooked to html5ever

Decisions: JS callbacks live in JS globals (`__tb_timeouts`); Rust holds integer ids so `Persistent<Function>` never crosses `spawn_blocking`. No `unsafe`.

Gotchas:
- `nix develop --command cargo …`
- `Page::run` must not nest inside another Tokio runtime
- html5ever `selectedcontent` option-clone: `KNOWN_UPSTREAM_DIVERGENCES`

Suggested skills: do-work (playbook: session-pickup), review
