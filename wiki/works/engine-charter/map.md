# Engine charter

Destination: Settled answers for the real holes (crate graph, event loop, template contents, transport injection, stealth honesty, layer ownership). Thinner crate law. No feature code. Wiki/ADR pass after this map.

Notes:

- Keep what already holds: generational arena, html5ever over html5gum, selectors in `dom`, ureq+native-tls for v1, html5lib as parser proof, `unsafe` deny in this repo.
- Smallest and light: HTML job queue we own; Tokio current-thread `rt`+`time` to park/wake (~+66 KB). No `full`/smol/axum/hyper. Sub-5 MB still the cap.
- Over-policing is the bug. Compile-error edges exist to stop CDP reaching around `browser`, not to invent a constitution.
- AGENTS.md still applies (WHATWG citations, size at milestones).
- Wiki/ADR rewrite is a follow-on pass, not this map's tickets.

Decisions so far:

- [Crate graph](./tickets/01-crate-graph.md): `dom`, `net`, `browser`. No `js` crate until QuickJS has a surface. Root depends on `browser` only. One law: CDP (when it exists) depends on `browser` alone.
- [Event loop](./tickets/02-event-loop.md): HTML jobs are ours; Tokio current-thread `rt`+`time` parks the page thread; ureq via `spawn_blocking`; no `full`/axum/hyper.
- [Template contents on Dom](./tickets/03-template-contents-on-dom.md): template → fragment map lives on `Dom`; sink integration-point flags do not.
- [Browser holds Agent](./tickets/04-browser-holds-agent.md): no `HttpTransport`; page HTTP is `net::Agent` on `browser`.
- [Stealth is later](./tickets/05-stealth-is-later.md): real milestone, later later; v1 stays ureq+native-tls.
- [Layer ownership](./tickets/06-layer-ownership.md): jar on `Agent`; `document.cookie` and `Content-Language` on the page; persistence is profile, not CDP.
- [parse_html stays in browser](./tickets/07-parse-html-stays-in-browser.md): engine crate; no fifth crate.

Not yet specified:

- (none — map complete; [spec](./spec.md) written)

Out of scope:

- Implementing QuickJS, navigation, CDP, or the stealth backend
- Reopening arena vs `Rc<RefCell>`, html5ever vs html5gum, selectors-in-dom, wreq
