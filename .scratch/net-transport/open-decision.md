# Net transport: open decision

Status: **UNDECIDED by the maintainer** (2026-08-25). Everything below is
measured evidence, not a choice. When a direction is picked, write
`docs/adr/0006-net-transport.md`, delete this file, and update the pointers
in `docs/size-budget.md`.

## The requirement that reframed net

Stealth means presenting browser-grade fingerprints on every layer a bot
blocker scores: TLS ClientHello (JA3/JA4), HTTP/2 settings and pseudo-header
order, header set/order/case. Interactive challenge solving (Turnstile and
alike) is an explicit non-goal; the contract is maximum passive trust plus
clean surfacing of challenges upward.

## Measured options (tuned profile, marginal vs empty-main baseline)

| | A: ureq + native-tls | B: ureq bridged to btls | C: hand-rolled h1/h2 on btls |
| --- | --- | --- | --- |
| net layer | +490 KB | +1797 KB | ~1300–1700 KB (est.) |
| full stack | ~2.5 MB | ~3.8 MB | ~3.6–3.7 MB |
| headroom left for CDP + a11y | large | ~1.2 MB | ~1.3–1.4 MB |
| JA4 presented | `t13d3011_…` OpenSSL | chrome knobs, h1-only | target: canonical `t13d1516h2_8daaf6152771` |
| live-gate score | 9/16 | 10/16 | 11 + 3 amb (wreq stand-in) |

(wreq itself is rejected at +4020 KB, stack ~6.0 MB; its patched-BoringSSL
fork survives standalone as `btls`/`btls-sys` 0.5.6.)

## Facts established by probes

- btls works standalone: sync over `std::net`, no tokio; safe-API knobs for
  grease, per-connection extension permutation (JA3 varies run-to-run,
  Chrome 110+ style), curves list accepting X25519MLKEM768 offered first on
  the wire.
- The ureq bridge needs NO fork or vendoring: public
  `Agent::with_parts(config, connector, resolver)` took a ~130-line bespoke
  Connector wholesale.
- Bridging caps out at: HTTP/1.1 only (ALPN must say h1), lowercase wire
  header names via `http::HeaderMap`. Those gaps are exactly where the
  matrix shows B losing to full coherence (tjx, bangkokair, bestbuy).
- The h2 implementation (HPACK, framing, flow control) is identical work in
  every branch that reaches canonical behavior; ureq contributes nothing
  there.
- Fork/absorb of ureq only becomes relevant for internals: case-preserving
  headers (deep surgery through its public API) or hosting h2 inside its
  h1-shaped transport model. At that point upstream merges are meaningless;
  it is absorb-and-own, with frozen-code maintenance duties.

## What the live matrix added

Full methodology and per-target table: `docs/testing.md`,
"Live bot-gate matrix". Headline: Akamai-class gates (tjx, bangkokair,
bestbuy) pass only under full coherence; g2/canadagoose block all columns
identically (IP reputation, outside client reach); zillow anomaly (B passed,
C failed) is single-sample noise until re-run.

## The actual decision

Pure priority call, no further measurement needed:

1. Ship working dials soonest: start at B (public APIs, zero commitment),
   keep C as the later milestone once h2 exists.
2. Build C directly: smallest binary, max control, most weeks of protocol
   work before anything dials.
3. Stay on A: rejected against the stealth goal; listed only for closure.

Sequencing note: choosing 1 does not foreclose 2; the bridge code remains a
reference implementation inside our own crate either way.
