# 08: Policy surfaces

Type: grilling

Question: Which policy knobs exist on `Agent` — redirects, proxies,
authentication — and what are their defaults?

Answer:

- **Proxy config adopted now**: one builder knob accepting an authority
  string (`http://user:pass@host:port`). Rationale: IP reputation is the
  enforcement layer bot gates apply hardest, proxy rotation is how stealth
  operators answer it, and ureq ships HTTP CONNECT support already — the
  knob is near-free today and painful to retrofit past the seam later.
  SOCKS5 stays disabled until someone measures its cost.
- **Redirect cap**: agent-level builder knob, default **20 follows**
  (Chrome's limit, not ureq's 10 — matching the fingerprint target costs
  nothing). Exceeded → `NetError::Limit(Redirects)`.
- **HTTP authentication: none in v1.** Headless has no prompt UI;
  embedders needing Basic/Negotiate set an `Authorization` header
  themselves. Recorded so the missing dialog is a decision, not a gap.
