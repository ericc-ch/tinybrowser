# 11: Types + send() over loopback

What to build: `net`'s entire public type surface — `Agent`/`AgentBuilder`,
`Method`, `Context`, `RequestBuilder`, `Response`, streaming `Body`,
case-preserving `HeaderMap`, `NetError` taxonomy, `url::Url` at the entry —
with `send()` working end-to-end over plain HTTP against a loopback
canned-response server. Includes the single ureq→net conversion point,
status-as-data (`http_status_as_error(false)`), the drop-cancels contract,
the recording-fake test server, and the first property tests.

Blocked by: None

Status: open

- [ ] GET against loopback server returns status, headers, and a streamed
      body through the public API only (no backend type escapes `net`)
- [ ] Non-2xx statuses arrive as `Ok(Response)`; transport failures arrive
      as `NetError::Transport(_)` variants
- [ ] Proptest: `HeaderMap::get` agrees with case-folded lookup; iteration
      preserves insertion order
- [ ] `Body::read_chunk` yields chunks incrementally; dropping
      `Response`/`Body` closes the connection (recording server observes
      the disconnect)
- [ ] Relative URLs rejected at `request()`; absolute `url::Url` accepted
- [ ] `cargo test` passes fully offline (no external network in CI);
      workspace lints clean
