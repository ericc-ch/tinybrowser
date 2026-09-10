# Net transport

Net v1 ships bare ureq 3 + native-tls so dials exist at the smallest binary cost (~+490 KB tuned, ~2.5 MB stack). Canonical-Chrome wire behavior (JA3/JA4, HTTP/2 settings and pseudo-header order, header case) stays the target and is deferred to a later hand-rolled h1/h2 stack on `btls`, not dropped.

Status: accepted

The API must survive that swap: public types are ours; ureq, native-tls, and tungstenite stay in `transport` and `websocket`.

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

## Consequences

Akamai-class gates fail until the stealth milestone ([ADR 0007](0007-engine-charter.md): later later). Probe knowledge (btls knobs, ureq `Agent::with_parts` bridge) stands as a reference. Size rows live in [size-budget.md](../researches/size-budget.md); the native-tls / OpenSSL-fingerprint objection returns when that milestone does. `net` stays blocking; the Browser-owned bounded executor runs it away from renderer threads.
