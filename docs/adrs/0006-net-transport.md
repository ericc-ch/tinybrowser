# Net transport

Net v1 shipped bare ureq 3 + native-tls so dials existed at the smallest binary cost (~+490 KB tuned, ~2.5 MB stack). Canonical-Chrome wire behavior (JA3/JA4, HTTP/2 settings and pseudo-header order, header case) stays the target.

Status: superseded for active transport by
[ADR 0019](0019-async-browser-runtime-and-io.md). This ADR remains the v1
transport and stealth-probe record. The public `net` type seam remains.

The API survives transport swaps: public types are ours, and backend types stay
private to `transport` and `websocket`.

## Options considered

Measured 2026-08-25 against real dials and the sixteen-target live gate below:

| | A: ureq + native-tls | B: ureq bridged to btls | C: hand-rolled h1/h2 on btls |
| --- | --- | --- | --- |
| net layer | +490 KB | +1797 KB | ~1300–1700 KB (est.) |
| live-gate | 9/16 | 10/16 | 11 + 3 amb (wreq stand-in) |

- **B now:** chrome TLS knobs flipped zillow, but h1-only ALPN and lowercase headers still lose Akamai-class gates; pays +1307 KB over A without reaching canonical behavior.
- **C now:** the only shape that meets stealth inside the budget; weeks of protocol work before anything dials. Declined on sequencing.
- **wreq:** realistic config +4020 KB → ~6.0 MB stack. Its patched-BoringSSL fork survives as `btls` 0.5.6 for the later milestone.
- **A forever:** not this decision. The live-gate delta is what is owed.

## Live-gate checkpoint (2026-08-25)

Throwaway client builds, not workspace tests. Re-run the matrix when the transport changes. Cells rot per site per day.

Columns: (1) bare ureq 3 + native-tls; (2) ureq bridged to btls 0.5.6 with chrome-ish TLS knobs, h1-only; (3) wreq Chrome148 as stand-in for the later hand-rolled stack.

Verdict: PASS = 200 with non-challenge body; FAIL(status) = >=400; FAIL(challenge) = interstitial markers; AMB(redirect) = 3xx with empty/small body; timeout counts as FAIL.

| Target | 1 | 2 | 3 |
| --- | --- | --- | --- |
| httpbin.org | PASS | PASS | PASS |
| nowsecure.nl | PASS | PASS | PASS |
| pastebin.com | PASS | PASS | PASS |
| g2.com | FAIL 403 | FAIL 403 | FAIL 403 |
| walmart.com | PASS | PASS | PASS |
| nike.com | PASS | PASS | AMB redirect |
| tjx.com | FAIL 403 | FAIL 403 | **PASS** |
| bangkokair.com | FAIL 403 | FAIL 403 | **PASS** |
| footlocker.com | PASS | PASS | PASS |
| canadagoose.com | FAIL 429 | FAIL 429 | FAIL 429 |
| stockx.com | FAIL 403 | FAIL 403 | AMB redirect |
| zillow.com | FAIL 403 | **PASS** | FAIL 403 |
| bestbuy.com | FAIL hang | FAIL hang | **PASS** |
| old.reddit.com | PASS | PASS | AMB redirect |

JA4: `t13d3011_…` (OpenSSL, no ALPN); `t13d2811h1_257f3020b3a2…` (chrome knobs, h1); `t13d1516h2_8daaf6152771…` (canonical Chrome, h2). g2 and canadagoose blocked every column (IP reputation). Only column 3 cleared Akamai-class first-request scoring.

## TLS backend revisit (2026-09-15)

The async transport shipped hyper-rustls with ring (ADR 0019). A throwaway
probe matrix then measured the four stacks: ureq + native-tls 761,712 bytes,
hand-rolled hyper + native-tls with ALPN h2 917,400, hyper + hyper-rustls/ring
1,847,440, and wreq + BoringSSL Chrome149 3,817,064. On that evidence the
shipping transport moved to **hyper-tls + native-tls** (system OpenSSL) for
both HTTP and WebSocket, keeping HTTP/2 by requesting ALPN `h2` ourselves.
Binary size drops from 6,582,320 to 5,621,760 bytes (−960,560) and rustls,
ring, and webpki leave the dependency graph.

Costs, recorded: TLS behavior is per-platform again (OpenSSL on Linux,
Security.framework on macOS, SChannel on Windows), the Linux build needs
OpenSSL headers, and the JA4 becomes `t13d3012h2_1d37bd780c83_…` — the
OpenSSL fingerprint of option A with h2 ALPN, not canonical Chrome. The
stealth milestone target is unchanged: Chrome-parity TLS knobs (`btls` or
equivalent) when anti-bot scoring becomes a product requirement, at the
measured cost in the table above.

## Consequences

Akamai-class gates fail until the stealth milestone. Probe knowledge (btls knobs,
ureq `Agent::with_parts` bridge) remains a reference. Size rows live in
[size-budget.md](../researches/size-budget.md). Net v2 uses async hyper-util and
hyper-rustls first, then replaces the private TLS connector with `btls` at the
stealth milestone. Tinybrowser does not hand-write HTTP/1.1 or HTTP/2.
