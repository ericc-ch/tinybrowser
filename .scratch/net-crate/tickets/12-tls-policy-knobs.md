# 12: Real TLS + policy knobs

What to build: HTTPS dials through native-tls, the proxy builder knob
(HTTP CONNECT authority string), agent-level timeouts, and the redirect
cap (default 20, Chrome parity) ending in `NetError::Limit`. Closes M1:
live example.com round-trip plus the fingerprint sanity check; binary
marginal measured and recorded.

Blocked by: 11

Status: open

- [ ] example.com over https round-trips through the public API
      (live smoke, manual run documented)
- [ ] Redirect cap: a counting loopback server proving follow-until-cap;
      exceeding it yields `NetError::Limit(_)`; default equals 20
- [ ] Proxy knob parses its authority string and reaches the agent config
      (unit-checked; no live proxy required)
- [ ] peet.ws JA4 echo confirms the OpenSSL `t13d3011_…` fingerprint —
      drift detection, not gate-passing
- [ ] Marginal measured vs empty-main baseline (expect ≈ +700 KB with
      ticket 11) and row recorded in docs/size-budget.md
