# Net crate v1

Destination: A decided public API for the `net` crate plus an implementation
plan with size checkpoints — derived from what `browser`'s navigation
lifecycle and the injected JS world (fetch, XHR, WebSocket) need, with a
hard seam so the deferred hand-rolled-btls stack replaces internals without
touching any consumer.

Notes:

- Stack fixed by [ADR 0006](../../adrs/0006-net-transport.md): v1 =
  ureq 3 + native-tls (~+490 KB tuned, dyn libssl.so.3). Stealth
  (canonical-Chrome wire behavior) is deferred to a later btls milestone,
  NOT abandoned — the API designed here must survive that swap.
- Edges per [ADR 0001](../../adrs/0001-workspace-crates-with-enforced-edges.md):
  `net` depends on nothing; `js` never depends on `net` (fetch injected via
  a consumer-side trait); `browser` is the fan-in point that wires them.
- Sync everywhere: `std::net`, no tokio (probe precedent + size-budget
  watchlist). Blocking IO is the default until a decision says otherwise.
- Repo rules: AGENTS.md (no unsafe; cite governing specs inline; measure
  binary size at milestones); [size-budget.md](../../researches/size-budget.md) discipline applies to every
  implementation milestone.
- Fact-finding (ureq 3 API truths: header case behavior, redirect hooks,
  connector/resolver surface) goes to sub-agents when a decision needs it;
  users are never asked what the repo or docs can answer.
- Standing tiebreaker (maintainer, 2026-08-25): for spec-shaped
  infrastructure, prefer servo-grade implementations over hand-rolling
  when the binary budget allows — the html5ever-over-html5gum logic,
  reaffirmed at the URL decision.

Decisions so far:

- [01: Destination and constraints](./tickets/01-destination-and-constraints.md):
  full build plan; consumers = navigation + fetch/XHR/WebSocket; hard seam,
  zero ureq leakage.
- [02: Core type model](./tickets/02-core-type-model.md): streaming Body,
  status-as-data, case-preserving HeaderMap, NetError taxonomy,
  agent-level config, `Context` on requests, final_url, no charset
  decoding, cancellation = drop.
- [03: JS-facing transport home](./tickets/03-js-transport-home.md):
  `HttpTransport` + plain types defined in `js`, implemented in `browser`
  over `net::Agent`, injected as `Box<dyn HttpTransport>` at runtime
  construction; sync method; XHR shares the trait.
- [04: URL handling](./tickets/04-url-handling.md): adopt servo's `url`
  crate (WHATWG-conformant); absolute URLs into `net`; relative/`<base>`
  resolution in `browser`; JS URL bindings via injection.
- [05: Cookie jar](./tickets/05-cookie-jar.md): own RFC 6265bis jar above
  the transport inside `send()`; backend cookie feature stays off;
  SameSite keyed off `Context` (Lax default); `document.cookie` methods on
  `Agent`; persistence deferred to CDP milestone.
- [06: WebSocket shape](./tickets/06-websocket-shape.md): tungstenite
  framing (probed +184 KB), one dial path owned by net shared with HTTP,
  caller owns socket post-upgrade (no pump thread), js-side injection
  trait, RFC 6455 close codes.
- [07: Navigation-facing API](./tickets/07-navigation-api.md): navigation
  is an ordinary `Context::Navigation` request; redirect chain deferred to
  CDP; referrer is a plain browser-set header; error pages belong to
  browser.
- [08: Policy surfaces](./tickets/08-policy-surfaces.md): proxy knob on the
  builder now (HTTP CONNECT; SOCKS unmeasured/off), redirect cap default 20
  (Chrome parity), no HTTP auth in v1.
- [09: Implementation sequencing](./tickets/09-implementation-sequencing.md):
  M1 dial (+~700 KB) → M2 jar → M3 ws (+184 KB); each milestone records its
  marginal; browser/js wiring is the follow-on effort.
- [10: Testing split](./tickets/10-testing-split.md): loopback e2e first,
  proptest for jar/HeaderMap, recording-fake server, no test-only exports;
  no gate-matrix re-run until the stealth milestone.

Not yet specified:

- (none — map complete; spec written; 11–14 done)

Implementation tickets (spec.md is the source of truth):

- [11: Types + send() over loopback](./tickets/11-types-send-loopback.md) —
  done 2026-08-25
- [12: Real TLS + policy knobs](./tickets/12-tls-policy-knobs.md) —
  done 2026-08-26 (closes M1)
- [13: Cookie jar above the transport](./tickets/13-cookie-jar.md) —
  done 2026-08-26 (closes M2)
- [14: WebSocket via shared dial path](./tickets/14-websocket-shared-dial.md)
  — done 2026-08-26 (closes M3)

[CONTEXT.md](../../CONTEXT.md) terms (**Hard seam**, **Context**, **Conversion point**) written
with ticket 14.

Out of scope:

- EventSource (excluded from the constraint set).
- HTTP disk cache (v1 headless ships none).
- h2 / canonical-Chrome wire work — belongs to the deferred stealth
  milestone (ADR 0006), not this effort.
