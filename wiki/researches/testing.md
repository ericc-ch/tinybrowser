# Testing Strategy

Two tiers, ordered by what they catch: parse correctness against the
spec-mandated tree, then unit/public-API tests for everything the suite
cannot see.

## Parse correctness: html5lib tree-construction suite (**landed 2026-08-25**)

Vendored as a pinned submodule (`third_party/html5lib-tests`, decisions and
acceptances in
[ADR 0005](../adrs/0005-html5lib-tree-construction-suite.md)); the harness at
`crates/browser/tests/html5lib.rs` runs every full-document case through the
public API under both scripting-flag settings and diffs byte-exactly against
the spec-mandated tree: **3549 cases green** (full-document plus fragment-context),
upstream's `<selectedcontent>` gap is a
documented divergence. This was the standing open item from the dom-layer
milestone; it covers exactly the misnesting/foster-parenting/adoption-agency
traps hand-written fixtures miss.

## Unit and public-API tests (landed)

- Arena mechanics: stale `NodeId` → clean miss, generation ticks on slot
  reuse, recycled slots never impersonate dead nodes.
- Content-model gates: pre-insert validity per WHATWG rule anchors, bulk
  moves (`reparent_children`) into documents answering to the same model.
- Selector matching over known trees, including state pseudo-class fidelity
  suites (`:checked`/`:disabled` inheritance, `:lang()` ranges, quirks-mode
  case regimes).
- Mutation storms auditing bidirectional link invariants at every step.
- Integration suites go through the public API only: no arena internals.

Verified at audit close: 73 tests green across the workspace, clippy pedantic
`-D warnings` clean, rustfmt clean.

## Live bot-gate matrix (manual checkpoint, first run 2026-08-25)

Net-layer stealth cannot be asserted from unit tests; it is a property
third-party gates either reward or punish on the wire. This checkpoint drives
throwaway client builds (not workspace code) through real targets and records
what passes. It is run by hand at net milestones; it is not wired into
`cargo test` because the targets are live third parties with their own WAF
moods.

**Client columns** (one build each):

1. bare ureq 3 + native-tls, default headers;
2. ureq 3 bridged to btls 0.5.6 via `Agent::with_parts` (chrome-ish TLS
   knobs: grease, permuted extensions, X25519MLKEM768 first), chrome-like
   header set, HTTP/1.1 only, lowercase wire names;
3. full-impersonation client (wreq 6-rc Chrome148 preset during this run),
   standing in for the future hand-rolled persona stack's wire behavior.

**Targets** (grouped by vendor): httpbin.org (control); tls.peet.ws and
tls.browserleaks.com (fingerprint echoes, always serve, report JA4 back);
nowsecure.nl, pastebin.com, g2.com (Cloudflare); walmart.com, nike.com,
tjx.com, bangkokair.com, bestbuy.com (Akamai class); footlocker.com (HUMAN);
canadagoose.com (PerimeterX); stockx.com (Kasada); zillow.com (aggressive
mixed scoring); old.reddit.com (light).

**Verdict rules**: PASS = 200 with non-challenge body; FAIL(status) = >=400;
FAIL(challenge) = interstitial markers (`Just a moment`, `cf-chl`,
`challenge-platform`); AMB(redirect) = 3xx with empty/small body (redirect
loop the single-shot GET cannot complete); timeout counts as FAIL. Each cell
also captures the JA4 the echo endpoints saw.

**Run of 2026-08-25** (single residential IP, GET only, no JS execution):

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

JA4 presented per column: `t13d3011_1d37bd780c83…` (OpenSSL, no ALPN);
`t13d2811h1_257f3020b3a2…` (chrome knobs, h1-only); `t13d1516h2_8daaf6152771…`
(canonical Chrome, h2 end-to-end). Readings that survived review:

- Akamai-class gates hard-score first-request TLS+headers; only column 3
  clears them (tjx, bangkokair, bestbuy).
- g2 and canadagoose blocked all columns identically: IP reputation and IP
  rate class, outside any client's influence.
- Column 2 passing zillow where column 3 failed is a single-sample anomaly;
  re-run before drawing conclusions.
- Nothing fired an active JS challenge against plain GETs; Turnstile-class
  behavior remains outside transport reach by design.

Cells rot per site per day; treat this as method plus snapshot, and re-run
the whole matrix when anything about the transport changes.

## Net live smokes (opt-in, landed 2026-08-26)

Offline `cargo test` never dials the internet. Ticket 12's HTTPS checks live
in `crates/net/tests/live.rs` and stay `#[ignore]`:

```
cargo test -p net --test live -- --ignored --nocapture
```

- `https://example.com/` round-trips through `net::Agent` (200 + page text).
- `https://tls.peet.ws/api/all` must still echo JA4 prefix `t13d3011` (OpenSSL
  native-tls drift check, not a bot-gate pass). Re-run when bumping ureq or
  native-tls.

## Explicitly deferred

- Full web-platform-tests corpus: needs harness machinery that belongs to
  later layers; adopt once js exists. The parser suite's `.dat` source
  moves to WPT at the same time (ADR 0005, "Upstream consolidation").
