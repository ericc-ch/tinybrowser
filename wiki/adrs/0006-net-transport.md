# Net transport

Net v1 ships bare ureq 3 + native-tls so dials exist at the smallest binary cost (~+490 KB tuned, ~2.5 MB stack). Canonical-Chrome wire behavior (JA3/JA4, HTTP/2 settings and pseudo-header order, header case) stays the target and is deferred to a later hand-rolled h1/h2 stack on `btls`, not dropped.

Status: accepted

The API must survive that swap: public types are ours; the backend lives behind one conversion point in `send()`.

## Options considered

Measured 2026-08-25 against real dials and the sixteen-target live gate ([testing.md](../researches/testing.md)):

| | A: ureq + native-tls | B: ureq bridged to btls | C: hand-rolled h1/h2 on btls |
| --- | --- | --- | --- |
| net layer | +490 KB | +1797 KB | ~1300–1700 KB (est.) |
| live-gate | 9/16 | 10/16 | 11 + 3 amb (wreq stand-in) |

- **B now:** chrome TLS knobs flipped zillow, but h1-only ALPN and lowercase headers still lose Akamai-class gates; pays +1307 KB over A without reaching canonical behavior.
- **C now:** the only shape that meets stealth inside the budget; weeks of protocol work before anything dials. Declined on sequencing.
- **wreq:** realistic config +4020 KB → ~6.0 MB stack. Its patched-BoringSSL fork survives as `btls` 0.5.6 for the later milestone.
- **A forever:** not this decision. The live-gate delta is what is owed.

## Consequences

Akamai-class gates fail until the stealth milestone ([ADR 0007](0007-engine-charter.md): later later). Probe knowledge (btls knobs, ureq `Agent::with_parts` bridge) stands as a reference. Size rows live in [size-budget.md](../researches/size-budget.md); the native-tls / OpenSSL-fingerprint objection returns when that milestone does. `net` stays blocking; the page thread uses Tokio only to park/wake and `spawn_blocking`.
