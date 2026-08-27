# 11: Types + send() over loopback

What to build: `net`'s entire public type surface — `Agent`/`AgentBuilder`,
`Method`, `Context`, `RequestBuilder`, `Response`, streaming `Body`,
case-preserving `HeaderMap`, `NetError` taxonomy, `url::Url` at the entry —
with `send()` working end-to-end over plain HTTP against a loopback
canned-response server. Includes the single ureq→net conversion point,
status-as-data (`http_status_as_error(false)`), the drop-cancels contract,
the recording-fake test server, and the first property tests.

Blocked by: None

Status: done (2026-08-25)

- [x] GET against loopback server returns status, headers, and a streamed
      body through the public API only (no backend type escapes `net`)
- [x] Non-2xx statuses arrive as `Ok(Response)`; transport failures arrive
      as `NetError::Transport(_)` variants
- [x] Proptest: `HeaderMap::get` agrees with case-folded lookup; iteration
      preserves insertion order
- [x] `Body::read_chunk` yields chunks incrementally; dropping
      `Response`/`Body` closes the connection (recording server observes
      the disconnect)
- [x] Relative URLs unrepresentable at `request()`: the `url::Url` entry
      type rejects base-less parsing by construction. Pinned by the type,
      not a test — ticket 10 forbids testing upstream crate behavior.
- [x] `cargo test` passes fully offline (no external network in CI);
      workspace lints clean

Amendments (2026-08-26, round-2 review):

- UA layering decided in `send()`: request-level `User-Agent` suppresses
  the agent default (not stacked with it). Repeated request-level UA
  entries still append (RFC 9110 §5.2). The backend auto-UA is always
  `None`; we never depend on ureq de-duplicating.
- Added loopback coverage: redirect chain (`final_url` + hop sequence),
  redirect cap → `Limit(Redirect)` bounded end-to-end, global timeout →
  `Transport(Timeout(Global))`, chunked transfer decoding,
  `Body::bytes(limit)` → `Limit(Size)`, fetch-accurate method tokens on
  the request line (`allow_non_standard_methods`), env `HTTP_PROXY`
  ignored (`.proxy(None)` until ticket 08's knob; proven via a child
  process, no `unsafe` env mutation).
- `send()` owns redirect following (ureq `max_redirects(0)`): 307/308
  replay method+body; 302 POST becomes GET; same-origin `Cookie` survives
  hops. Ticket 02 status-as-data wording amended to "final status after
  redirect policy".
- Added `tests/token_grammar.rs`: negative paths of header name/value and
  method token grammars (RFC 9110 §5.1/§5.5/§9.1).
- Removed tests of third-party/cosmetic behavior per ticket 10 rules:
  relative-URL rejection via `url::Url` (type-guaranteed), Display-prefix
  stability (no caller matches strings).
