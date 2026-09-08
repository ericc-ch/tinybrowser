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

The suite keeps only high-signal public-boundary gates: the html5lib corpus,
an independent DOM mutation model, selector state matrices, browser page-loop
journeys, and compact HTTP/WebSocket/cookie/error transcripts. The current
workspace lists **20 tests** (19 active plus one ignored corpus-dump helper).
Integration suites do not reach into arena internals.

The live bot-gate matrix below remains a manual checkpoint (first run
2026-08-25):

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

## WPT runner (landed; testharness bar still open)

The full WPT tree is pinned and driven by in-process classic WebDriver
(`./tools/wpt/run`). HTTPS tests are off (`--ssl-type none`) until cert
trust exists. html5lib-tests remain the parser gate. A passing testharness
file through that runner is the next evidence, not a claim of this
landing ([ADR 0008](../adrs/0008-wpt-via-webdriver.md)).
