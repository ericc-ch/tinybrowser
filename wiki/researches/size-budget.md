# Size Budget

Goal: **sub-5MB stripped x86_64 binary** (AGENTS.md Vision). Measured 2026-08-21 and 2026-08-23, rustc 1.98.0, Linux.

Reproduce each dependency row with a probe binary that really exercises it
(tokenize, dial, JS eval, parse+query). Marginal = binary delta vs an
empty-`main` build of the same profile. Probes are throwaway; each
checkpoint records its marginal here. Future parse+query checkpoints drive
the real adapter (`browser::parse_html`).

## Measured marginal costs

| Component                                        | default `release` | tuned¹  |
| ------------------------------------------------ | ----------------- | ------- |
| baseline (empty main)                            | 345 KB            | 290 KB  |
| html5gum 0.8 (tokenizer only)                    | +388 KB           | +272 KB |
| **html5ever 0.39 + tree builder** (rcdom sink)   | +941 KB           | +840 KB |
| selectors 0.26 + cssparser (on top of html5ever) | +115 KB           | +75 KB  |
| ureq 3 + native-tls (dyn libssl.so.3)            | +753 KB           | +482 KB |
| rquickjs 0.12 (quickjs-ng, eval + limits)        | +1247 KB          | +776 KB |

¹ `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`, stripped

## Milestone: dom v1 measured (2026-08-23)

A real Wikipedia page (405 KB HTML → 4,051 live elements) went through the
dom stack, then selector queries of every common shape (`a[href]`,
descendant lists, id/class, attribute ops, `:nth-child`, comma lists) ran
against the result.

| Component                                            | default `release` | tuned¹  |
| ---------------------------------------------------- | ----------------- | ------- |
| baseline (empty main, re-measured)                   | 448 KB            | 287 KB  |
| **dom v1**: arena + selectors + cssparser + html5ever + markup5ever | +1272 KB | **+932 KB** |

Against the pre-measurement estimate for the same stack (+941 KB +115 KB = +1056 KB release / +840 KB +75 KB = +915 KB tuned): tuned landed within ~2% (+17 KB); release ran +216 KB over; the delta carries dom's own storage/search code plus query execution, which the estimates omitted. Accepted: no regression to justify.

## Milestone: net transport probes (2026-08-25)

The stealth requirement landed before net v1: bot blockers (Cloudflare and
alike) fingerprint three layers, TLS ClientHello (JA3/JA4), HTTP/2
settings/pseudo-header order, and header set/order, so "bytes in, bytes out"
must mean *browser-shaped* bytes. Probes dial `example.com` or
`tls.peet.ws/api/all` live; rustc 1.98.0; tuned profile as committed to the
root Cargo.toml this same day (the doc previously claimed those flags lived
there; they had never actually been added).

| Component | default `release` | tuned¹ |
| ------------------------------------------------- | ---------------- | ------- |
| baseline (empty main, re-measured) | 345 KB / 290 KB | 290 KB |
| ureq 3 + native-tls (dyn libssl.so.3), real dial | — | +490 KB |
| boring 4 raw TLS, handshake only | — | +1220 KB |
| **btls 0.5.6 standalone**, peet.ws JA4 round-trip | — | +1302 KB |
| ureq 3 (no built-in TLS) bridged to btls via `Agent::with_parts`, live dials incl. peet.ws echo | — | +1797 KB |
| wreq 6.0.0-rc core (defaults) | — | +2545 KB |
| wreq 6.0.0-rc + util presets | — | +3668 KB |
| wreq 6.0.0-rc realistic (+cookies,gzip,brotli,zstd,prefix,preset) | — | +4020 KB |
| **url 2.x**, WHATWG parse incl. IDN host + join + serialize | — | +197 KB |
| **tungstenite 0.30** (handshake feature only, no TLS), real loopback dial + frames + close | — | +184 KB |

(url and tungstenite probed 2026-08-25 for the net-crate API effort
([ADR 0006](../adrs/0006-net-transport.md)); same rustc 1.98.0, tuned profile,
baseline re-measured at 290 KB — identical to the row above. In one binary
they land +336 KB combined: ~53 KB shared-dependency overlap.)

btls knob verification (safe API, sync over `std::net`, no tokio):
`set_permute_extensions` shuffles extension order per connection, JA3 hash
varies run-to-run while JA4 stays stable, exactly the Chrome 110+ behavior;
`set_grease_enabled` applies; `set_curves_list` accepts `X25519MLKEM768` and
offers group 4588 first with a real key share, matching current Chrome. The
default ClientHello is recognizably non-browser (`t13d2811h1_257f3020b3a2`)
until persona presets are fed in, which is expected; preset data is work,
not risk.

### Live-gate matrix snapshot (2026-08-25)

Sixteen real targets across vendors (Cloudflare, Akamai, HUMAN,
PerimeterX, Kasada) driven by three throwaway client builds matching the
option space below; full methodology, target list, and verdict rules live in
[testing.md](testing.md). Headline results:

- Bare ureq+native-tls clears 9/16; the OpenSSL fingerprint (`t13d3011_…`,
  no ALPN) hard-fails tjx, bangkokair, stockx, zillow, bestbuy.
- The ureq+btls bridge clears 10/16: chrome-ish TLS knobs alone flipped
  zillow from 403 to 200, but h1-only ALPN and lowercase headers still lose
  tjx/bangkokair/bestbuy/stockx.
- Full impersonation coherence (wreq Chrome148, stand-in for the hand-rolled
  client's wire behavior) clears 11 plus 3 ambiguous redirect flows, and is
  the only column presenting canonical Chrome (`t13d1516h2_8daaf6152771`)
  while speaking h2 end-to-end.
- g2.com and canadagoose blocked all three columns identically: that layer
  is IP reputation, outside any client's reach.

Cells rot per site per day; treat the matrix as a checkpoint method, not a
standing truth.

### Decision consequences

- **wreq rejected wholesale** (review decision, 2026-08-25): the realistic
  configuration (+4020 KB) puts the full stack at ~6.0 MB against the 5 MB
  budget with CDP and a11y still unaccounted. Its patched-BoringSSL fork
  survives independently as the `btls`/`btls-sys` crate family (0.5.6),
  which is what the probe above exercises.
- **Transport direction closed** (maintainer decision, 2026-08-25,
  [ADR 0006](../adrs/0006-net-transport.md)): net v1 ships as *bare ureq +
  native-tls* (+490 KB net, ~2.5 MB stack, 9/16 gates). Stealth is deferred,
  not dropped: canonical-Chrome wire behavior stays the target, owned by the
  later *hand-rolled h1/h2 on btls* milestone (≈1.3–1.7 MB net, the only
  shape reaching it within budget). The *ureq→btls bridge* shape (+1797 KB,
  h1-only ceiling) was declined — it buys neither ship-soonest nor
  coherence. Full evidence and per-shape gate scores are in the ADR; the
  sizes stay in the probe table above.
- **ureq-over-btls bridging needs no vendoring**: ureq exposes
  `Agent::with_parts(config, connector, resolver)` for bespoke Connectors;
  the bridge is ~130 lines in our own crate. Fork/absorb becomes relevant
  only for internals (case-preserving headers, hosting an h2 stack inside
  ureq's transport model). Dormant now that the bridge shape is declined;
  relevant again only if the deferred stealth milestone wants it.
- The earlier native-tls decision above is reinstated for net v1 by
  [ADR 0006](../adrs/0006-net-transport.md), with the stealth objection to it
  deferred alongside the goal itself (see the ADR).

## Milestone: net M1 dial (2026-08-26)

Ticket 12 closed M1: `net::Agent` over ureq 3 + native-tls (dyn libssl.so.3) plus
servo `url`, with `send()` following redirects (cap 20) and an HTTP CONNECT
proxy knob. Probe binary actually GETs `https://example.com/` through the
public API. rustc 1.98.0; tuned profile as in the root Cargo.toml.

| Component | default `release` | tuned¹ |
| ------------------------------------------------- | ---------------- | ------- |
| baseline (empty main, re-measured) | — | 284 KB |
| **net M1**: Agent + HTTPS `send()` (ureq native-tls + url + redirect/proxy policy) | — | **+679 KB** |

Against the M1 estimate (+490 KB ureq/native-tls +197 KB url ≈ +700 KB): tuned
landed −21 KB (shared-dep overlap plus our own types). Accepted.

Live smokes (opt-in, `cargo test -p net --test live -- --ignored`): example.com
200; tls.peet.ws JA4 still `t13d3011_…` (OpenSSL native-tls, drift check).

## Milestone: net M2 cookie jar + M3 WebSocket (2026-08-26)

Ticket 13 put an RFC 6265bis jar above the transport (no new dependency). Ticket 14
replaced ureq's `DefaultConnector` with a net-owned `dial::open` shared by
`send()` and `RequestBuilder::upgrade`, then linked tungstenite 0.26 (`handshake` only).
Probes call the public API against `127.0.0.1:1` (enough to keep TLS and the
WebSocket framing from being stripped). rustc 1.98.0; tuned profile as in the root
Cargo.toml. Empty-main re-measured at 284 KB (290720 bytes), matching M1.

| Component | default `release` | tuned¹ |
| ------------------------------------------------- | ---------------- | ------- |
| baseline (empty main, re-measured) | — | 284 KB |
| **net M2/M3 HTTPS `send()`** (shared dial + jar, no WS call in the probe) | — | **+490 KB** |
| **net M3** same probe plus `RequestBuilder::upgrade` | — | **+603 KB** |
| tungstenite marginal (M3 minus HTTPS-only) | — | **+113 KB** |

Standalone tungstenite was probed at +184 KB (2026-08-25). In this crate it
lands +113 KB on top of the HTTPS agent (~71 KB overlap with existing deps).
HTTPS-only dropped from M1's +679 KB to +490 KB because `send()` no longer
pulls ureq's default TCP/TLS connector stack.

Cookie jar itself added no crate; mid-implementation it was ~+20 KB of our
code on the old M1 connector. Accepted.

## Stack totals (tuned profile)

| Stack                                                  | Total       | Headroom to 5 MB |
| ------------------------------------------------------ | ----------- | ---------------- |
| ureq(native-tls) + quickjs-ng + **html5ever** ← chosen | **2.26 MB** | ~2.74 MB         |
| same but html5gum instead                              | 1.73 MB     | ~3.27 MB         |
| any of the above on default `release`                  | 2.7–3.3 MB  | n/a              |
| chosen + **dom v1** (2026-08-23, components re-summed) | **~2.42 MB**| ~2.58 MB         |
| same + **net M1 dial** (2026-08-26; ureq probe swapped for measured crate, +189 KB over the +490 KB ureq row) | **~2.61 MB** | ~2.39 MB |
| same + **net M3** (2026-08-26; shared dial + jar + tungstenite, +113 KB over the +490 KB ureq row) | **~2.53 MB** | ~2.47 MB |
| dom v1 + quickjs + **net on btls, hand-rolled h1/h2** (est., pending preset work, see 2026-08-25 probes) | ~3.6–3.7 MB | ~1.3–1.4 MB |
| dom v1 + quickjs + **net as ureq bridged to btls** (measured bridge) | ~3.8 MB | ~1.2 MB |

## Decisions

- **html5ever over html5gum** (+~570 KB): buys the complete HTML5 tree-construction algorithm (insertion modes, foster parenting, adoption agency). html5gum is tokenizer-only; hand-rolling tree construction is weeks of fiddly spec work. Maturity wins under a 5MB budget.
- **Tuned profile from day one**: default release costs +400–850KB for nothing. The flags are set once in the root Cargo.toml.
- **native-tls, dynamically linked**: TLS lives in system `libssl.so.3`; we ship only glue (~482 KB tuned). Static rustls would add ~+2.0 MB: rejected. Consequence: target machine needs OpenSSL 3 installed (near-universal on Linux). *(2026-08-25: superseded within the day by the stealth requirement — its probes showed an OpenSSL ClientHello among the most-flagged fingerprints — then reinstated for net v1 by [ADR 0006](../adrs/0006-net-transport.md) with stealth deferred to the hand-rolled-btls milestone. The objection returns when that milestone does.)*
- **panic = unwind kept**: abort saves only ~39 KB but kills `catch_unwind`, which every JS-exposed op needs so a Rust panic degrades to a JS error instead of unwinding through QuickJS's C frame.
- **selectors later is cheap**, confirmed at the dom-v1 checkpoint: the whole dom layer (arena + selector engine + parser stack) measured +932 KB tuned, within ~2% of the html5ever+selectors estimates it subsumes (see Milestone section).
- **Old servo stack (html5ever + selectors + cssparser as the _core_) was never the problem**: the old repo's total was bloat elsewhere. The parser swap alone does not hit 5MB; discipline at every milestone does.

## Watchlist (what can still blow the budget)

- Children-representation collapse (inline/heap split → plain `Vec<NodeId>`,
  2026-08-24, per ADR 0002): expected ≈0 binary impact (no dependency change);
  confirm at the next parse+query probe.

- DOM→JS binding glue: hundreds of rquickjs classes add up; keep dispatch tables data-driven.
- CDP server: tokio-tungstenite-style async stack is expensive; prefer a lean HTTP+WebSocket impl on `std::net`.
- A11y walker (accname computation, role mapping): budget ~100–200 KB, fine, but measure.
- When the deferred stealth milestone lands net on btls ([ADR 0006](../adrs/0006-net-transport.md)): pin the crate family like html5ever (its BoringSSL fork is wreq-ecosystem); impersonation presets go stale with every Chrome release — a stale preset is itself a detection signal, so bump discipline applies to persona tables, not just crates.
- Re-measure marginals at every milestone; regressions must justify themselves in bytes.
