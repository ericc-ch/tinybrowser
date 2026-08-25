# ADR 0006: net transport

Date: 2026-08-25
Status: Accepted. Closes the open decision collected in
`.scratch/net-transport/open-decision.md` (deleted with this ADR). Reinstates
native-tls for net v1 (see the Decisions list in
[size-budget.md](../size-budget.md)); the stealth requirement moves to a
later milestone instead of dying.

## Context

The stealth requirement landed before net v1 (2026-08-25): bot blockers
fingerprint three layers — TLS ClientHello (JA3/JA4), HTTP/2 settings and
pseudo-header order, header set/order/case — so "bytes in, bytes out" had to
mean *browser-shaped* bytes, with interactive challenge solving (Turnstile
and alike) an explicit non-goal. Three shapes were measured against real
dials and a sixteen-target live gate matrix ([testing.md](../testing.md),
tuned profile, marginal vs empty-main baseline):

| | A: ureq + native-tls | B: ureq bridged to btls | C: hand-rolled h1/h2 on btls |
| --- | --- | --- | --- |
| net layer | +490 KB | +1797 KB | ~1300–1700 KB (est.) |
| full stack | ~2.5 MB | ~3.8 MB | ~3.6–3.7 MB |
| JA4 presented | `t13d3011_…` OpenSSL | chrome knobs, h1-only | canonical `t13d1516h2_8daaf6152771` |
| live-gate score | 9/16 | 10/16 | 11 + 3 amb (wreq stand-in) |

Facts established by the probes:

- wreq is rejected on size (+4020 KB realistic → ~6.0 MB stack); its
  patched-BoringSSL fork survives standalone as `btls`/`btls-sys` 0.5.6.
- btls works standalone over sync `std::net`, no tokio: safe-API knobs for
  grease, per-connection extension permutation (JA3 varies run-to-run,
  Chrome 110+ style), curves list accepting X25519MLKEM768 offered first.
- The ureq bridge needs no fork or vendoring (~130-line bespoke Connector
  via public `Agent::with_parts(config, connector, resolver)`), but caps at
  HTTP/1.1-only ALPN and lowercase wire header names — exactly where
  Akamai-class gates stay lost (tjx, bangkokair, bestbuy).
- The h2 implementation (HPACK, framing, flow control) is identical work in
  every branch that reaches canonical behavior; ureq contributes nothing
  there.

## Decision

**Option A ships net v1: bare ureq 3 + native-tls — and stealth is
deferred, not dropped.** Presenting canonical-Chrome wire behavior remains
the project target, owned by option C as a later milestone once h2 exists.

The call is priority, not feasibility: A is the soonest working dials at the
smallest binary cost (~2.5 MB stack leaves the largest headroom, ~2.5 MB,
for CDP + a11y), while C is weeks of protocol work before anything dials.
B was declined as the middle that pays +1307 KB over A without reaching
canonical behavior anywhere.

Declined alternatives, recorded for closure:

- *B, ureq bridged to btls*: chrome TLS knobs alone flipped zillow
  (403→200), but h1-only ALPN and lowercase headers still lose tjx,
  bangkokair, bestbuy, stockx — it buys neither ship-soonest nor coherence.
- *C now*: the only shape meeting the stealth goal within budget; declined
  on sequencing, not merit.
- *A forever*: explicitly not what this ADR decides. The deferral has no
  deadline, but the live-gate delta (9/16 vs 11 + 3 ambiguous) is the
  standing measure of what is owed. (g2/canadagoose block all columns
  identically regardless: IP reputation, outside any client's reach.)

Nothing here forecloses C: the probe knowledge (btls knobs, bridge shape,
preset requirements) stands, and the bridge pattern stays a reference
implementation for our own crate either way. What C adds over today's
evidence: h2 with coherent settings/pseudo-header order, HPACK, flow
control, case-preserving headers, persona presets — gated by a re-run of
the live matrix, including the single-sample zillow anomaly (B passed,
C failed).

## Consequences

- [size-budget.md](../size-budget.md): transport direction closed there;
  the probe-size rows and watchlist wording updated to match.
- The earlier native-tls entry in the size-budget Decisions list is
  reinstated for v1; its OpenSSL-fingerprint objection returns exactly when
  the stealth milestone does.
- Until the stealth milestone lands, Akamai-class gates are expected to
  fail. Surfacing challenges upward cleanly stays part of the contract; it
  does not depend on transport.
