# Testing Strategy

Web-visible platform behavior is WPT (`./tools/wpt/run`). `cargo test` is
tinybrowser-specific: parser corpus, product adapters, page pump, and
transport. Browser-crate JS/DOM cargo tests are stand-ins until the first
testharness file is green; they are not a second web suite
([ADR 0008](../adrs/0008-wpt-via-webdriver.md)).

## Parse correctness: html5lib tree-construction suite (**landed 2026-08-25**)

Vendored as a pinned submodule (`third_party/html5lib-tests`, decisions and
acceptances in
[ADR 0005](../adrs/0005-html5lib-tree-construction-suite.md)); the harness at
`crates/browser/tests/html5lib.rs` runs every full-document case through the
public API under both scripting-flag settings and diffs byte-exactly against
the spec-mandated tree: **3549 runs** with **10 accepted divergences**
(full-document plus fragment-context). The accepted set is html5ever’s
`<selectedcontent>` option-clone (`webkit02.dat` #44–47) and select-fragment
`<input><option>` (`tests_innerHTML_1.dat` #75), both scripting flags, listed
in `KNOWN_UPSTREAM_DIVERGENCES` ([ADR 0005](../adrs/0005-html5lib-tree-construction-suite.md)).
This was the standing open item from the dom-layer milestone; it covers
exactly the misnesting/foster-parenting/adoption-agency traps hand-written
fixtures miss.

## Unit and public-API tests (landed)

`cargo test` covers product and transport boundaries: daemon lock, CDP
flatten/method-not-found/`Browser.close`, WebDriver one-session and
`DELETE /session` leaving tabs, unsupported click, page pump vs unrelated
fetch, cookie file mode, CLI flag errors, `net::Agent` cookies/loopback/WS,
and inbound HTTP/1.1 on `http1`. Counted 2026-09-10
from workspace `#[test]` items: **68 tests** (67 active plus one ignored
corpus-dump helper in `crates/browser/tests/html5lib.rs`).
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

The full WPT tree is pinned and driven by classic WebDriver (`./tools/wpt/run`) over `BrowserHandle`. Isolation is a fresh temporary `XDG_RUNTIME_DIR` and `XDG_DATA_HOME` per WebDriver endpoint (`tools/wpt/tinybrowser_wpt.py`). HTTPS tests are off (`--ssl-type none`) until cert trust exists.
html5lib-tests remain the parser gate until testharness runs `html/syntax/parsing/`. Browser-crate JS/DOM cargo tests (`createElementNS`, `nodeName`, `instanceof`) are stand-ins until the first testharness file is green; delete them then. A passing testharness
file through that runner is the next evidence, not a claim of this landing.
