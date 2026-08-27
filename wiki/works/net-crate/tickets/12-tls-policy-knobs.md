# 12: Real TLS + policy knobs

What to build: HTTPS dials through native-tls, the proxy builder knob
(HTTP CONNECT authority string), agent-level timeouts, and the redirect
cap (default 20, Chrome parity) ending in `NetError::Limit`. Closes M1:
live example.com round-trip plus the fingerprint sanity check; binary
marginal measured and recorded.

Blocked by: 11

Status: done (2026-08-26)

- [x] example.com over https round-trips through the public API
      (live smoke, manual run documented)
- [x] Redirect cap: a counting loopback server proving follow-until-cap;
      exceeding it yields `NetError::Limit(_)`; default equals 20
- [x] Proxy knob parses its authority string and reaches the agent config
      (unit-checked; no live proxy required)
- [x] peet.ws JA4 echo confirms the OpenSSL `t13d3011_…` fingerprint —
      drift detection, not gate-passing
- [x] Marginal measured vs empty-main baseline (expect ≈ +700 KB with
      ticket 11) and row recorded in wiki/researches/size-budget.md

Run notes:

- Offline: `cargo test -p net` (loopback HTTPS-to-plaintext pins
  `Transport(Tls)`; redirect/proxy tests in `send_loopback.rs`).
- Live: `cargo test -p net --test live -- --ignored --nocapture`
  (2026-08-26: example.com 200; peet.ws JA4 still `t13d3011`).
- Size: empty-main 284 KB tuned; net M1 dial probe +679 KB tuned
  (rustc 1.98.0). Estimate was ≈+700 KB.
