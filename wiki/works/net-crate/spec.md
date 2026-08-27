# Net crate v1

Problem: tinybrowser has no way to fetch anything. Navigation needs
documents, injected page JavaScript needs fetch/XHR/WebSocket, and every
layer must fit the sub-5MB binary goal. The stealth requirement
(canonical-Chrome wire fingerprints) is deferred but not abandoned, so the
network API must survive replacing its internals with a hand-rolled
BoringSSL stack later — without any consumer changing.

Solution: a small sync `net` crate owning its entire public type surface,
built on ureq 3 + native-tls for v1 (ADR 0006). Requests carry an initiator
`Context`; responses treat every HTTP status as data; bodies stream; the
cookie jar lives above the transport so no backend swap can touch it.
`js` defines what it needs as traits; `browser` implements them over `net`
at the fan-in point.

User stories:

1. As an embedder, I want `Agent::request(...).send()` to return any HTTP
   response as data, so my navigation renders error pages like normal ones.
2. As page JavaScript, I want `fetch()`/XHR to run through one injected
   transport, so the JS world never depends on the network crate directly.
3. As page JavaScript, I want WebSockets with standard close codes, so
   live pages behave like in a real browser.
4. As an embedder, I want cookies stored and replayed per RFC 6265bis with
   SameSite honored by initiator context, so login flows and consent
   redirects work.
5. As the maintainer, I want each milestone's binary cost measured, so the
   5 MB budget never silently erodes.

Implementation decisions:

- Hard seam: every public type is ours (`Agent`, `Response`, `Body`,
  `HeaderMap`, `NetError`, `Context`); ureq types exist only inside
  `send()`'s single conversion point; statuses-as-data via
  `http_status_as_error(false)`.
- Streaming-first `Body`; dropping `Response`/`Body` closes the connection
  (that IS cancellation). No charset decoding in net — raw bytes +
  Content-Type string go up (WHATWG encoding sniffing belongs to the parse
  pipeline).
- `HeaderMap` preserves case and insertion order; lookup is
  ASCII-case-insensitive (RFC 9110 §5.1). Under v1, received headers
  populate lowercase (all ureq exposes); the stealth swap upgrades
  fidelity, signatures never move.
- URLs: servo's `url` crate (probed +197 KB tuned); absolute URLs into
  net; relative/`<base>` resolution in browser; JS URL bindings via the
  same injection pattern.
- Cookie jar: our own RFC 6265bis implementation above the transport —
  harvest Set-Cookie / build Cookie header inside `send()`; backend cookie
  feature off; SameSite keyed off `Context` (Lax default); persistence
  deferred to CDP.
- WebSocket: tungstenite framing behind a measured size gate (+184 KB);
  handshake through `RequestBuilder::upgrade` with `Context::WsHandshake`;
  one dial path and one outbound prepare shared with HTTP `send()`;
  caller owns the socket post-upgrade (no background pump). `browser`/`js`
  map DOM events onto the handle.
- JS boundary: `HttpTransport` trait + plain types defined in `js`,
  implemented in `browser` over `net::Agent`, injected as
  `Box<dyn HttpTransport>` at runtime construction (same inversion as
  html5ever's TreeSink).
- Policy: proxy builder knob now (HTTP CONNECT; SOCKS unmeasured), redirect
  cap default 20 (Chrome parity), no HTTP auth in v1.
- Milestones: M1 dial (+~700 KB expected) → M2 jar → M3 ws; each records
  its marginal in [size-budget.md](../../researches/size-budget.md).

Testing decisions:

- Priority per code-conventions: loopback end-to-end through the public
  API first (canned-HTTP TCP server), integration seams next (cookie
  replay across sends, redirect chains, drop-cancels observed by the
  server), then property tests for pure domain modules (jar match rules;
  HeaderMap case/order invariants) via `proptest` (dev-dep only).
- The loopback server is a recording fake: assertions check captured
  requests, never spies or call counts. No test-only exports anywhere.
- Live coverage: manual example.com smoke; one peet.ws JA4 echo as
  fingerprint-drift sanity. No bot-gate matrix re-run until the stealth
  milestone (ADR 0006 owns it).

Out of scope:

- EventSource, HTTP disk cache, h2/canonical-Chrome wire work (deferred
  stealth milestone, ADR 0006), SOCKS proxy (until measured), HTTP auth,
  jar persistence (CDP milestone), browser/js wiring (follow-on effort).
- Fetch `mode` / CORS / preflight / opaque responses, `credentials`
  omit/include/same-origin, and referrer policy: those belong in
  `browser`/`js` above this dial, not in `net`.

Notes:

- Decisions 01–10: [tickets/](./tickets/)
- Ground truth used: ureq 3.4 docs (docs.rs), Firefox nsIChannel.idl +
  FetchDriver.h (raw.githubusercontent.com per AGENTS.md), RFC 9110 §5.1,
  RFC 6265bis §5.3–5.4, WHATWG fetch #http-network-or-cache-fetch.
