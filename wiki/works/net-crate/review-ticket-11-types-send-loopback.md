# Code review: ticket 11 — types + `send()` over loopback

Reviewed: 2026-08-25 (uncommitted working tree vs `HEAD`). Findings re-checked
against the tree on 2026-08-26.

Scope: `crates/net/src/*` (new), `crates/net/tests/*` (new), `lib.rs` /
`Cargo.toml` diffs, ticket bookkeeping in `wiki/works/net-crate/`.

Verified before reviewing:

- `cargo test -p net --offline`: passes (12 integration + 1 property).
- `cargo clippy -p net --all-targets`: clean against the deny-by-default
  workspace lints. Zero `unsafe`.
- Two throwaway wire probes against the loopback server, checking things the
  committed tests cannot catch:
  1. **Configured UA + manual UA injection → duplicate `User-Agent`?** No.
     One header on the wire (`UA-COUNT=1`): ureq suppresses its configured
     UA when the request already carries one. Suspicion cleared — but note
     `builder_user_agent_rides_the_wire` only checks *first* match, so it
     would not have caught duplication.
  2. **Fragment-carrying URL** (`…/page#frag`)? Dials fine, wire target is
     `/page`, final URL drops the fragment. Cleared — see S3 for why it is
     still flagged.

## Spec Compliance

Ticket 11's six acceptance boxes are all genuinely delivered and pinned by
tests — including the honest amendment of the ticket-02 header-case contract
when reality disagreed with it.

### S1 — `Method` normalization contradicts the very spec it cites (medium-high)

`method.rs:33-36` claims tokens are uppercased with "the same normalization
`fetch()` applies (WHATWG fetch, #dom-request)", and `Method::parse`
(`method.rs:89-105`) blanket-uppercases **everything**. The fetch standard
does not do that. Per fetch §2.2.1 Methods
(<https://fetch.spec.whatwg.org/#methods>):

> To normalize a method, if it is a byte-case-insensitive match for `DELETE`,
> `GET`, `HEAD`, `OPTIONS`, `POST`, or `PUT`, byte-uppercase it.

That list is exhaustive. The spec even calls out the exact case this code
gets wrong: *"Using `patch` is highly likely to result in a `405`"* — `PATCH`
is deliberately excluded, and extension tokens keep their case (*"`Egg` or
`eGg` would be fine"*). So:

- `Method::parse("patch")` → wire `PATCH`; Chrome's fetch sends `patch`
- `Method::parse("propfind")` → wire `PROPFIND`; Chrome sends `propfind`

For a project whose entire reason to exist is canonical-Chrome wire bytes,
this is a fingerprint divergence baked into the boundary type that
page-JavaScript method strings will flow through.

Fix now: uppercase only the six listed methods, store extension tokens
verbatim — or record an explicit deviation decision. Do not leave a wrong
spec citation standing over wrong behavior.

### S2 — Ticket 02's builder contract silently shrunk (medium-low)

Ticket 02: "Config lives on `Agent` (builder: timeouts global+per-call,
redirect cap, **buffer limits**, user-agent)." `AgentBuilder`
(`agent.rs:19-26`) implements three of four; buffer limits simply don't
exist, and size capping was pushed onto every caller of
`Body::bytes(limit)` / `text(limit)`. Maybe that is the right design — but
the header-case correction proved the process: when implementation deviates
from a recorded decision, amend the ticket. This deviation is unrecorded.
Amend ticket 02 or add the knob.

(The spec's "proxy builder knob now" is ticket 08 / M1 territory — not held
against this diff.)

### S3 — Fragment handling is correct by accident and owned by nobody (medium-low)

Probe result: `…/page#frag` dials as `/page`, and `final_url()` comes back
without the fragment. Both match browser behavior (fetch
#http-network-or-cache-fetch strips fragments pre-network; `Response.url`
excludes them). But nothing in this crate decides that — it falls out of
ureq's URI round-trip — and `response.rs:141-145` cites the spec as if it
were enforced here. Navigation to anchored URLs is the common case for this
crate's biggest consumer, and the stealth swap replaces exactly this
backend.

Strip the fragment explicitly at `send()` / `from_backend` (or document it
as a v1-backend caveat like the lowercase-header one) and pin it with a
test. Right now the contract lives in ureq's source code, not ours.

### S4 — Test doesn't assert its own headline claim (low)

`send_loopback.rs:356-358`: "Nothing reached the wire: … the server saw no
connection." No assertion backs that — `server.requests()` should be
asserted empty. As written, a regression that dials anyway passes the test
while the comment lies about what was proven.

## Coding Standards

### C1 — Verbatim duplicated logic across two modules (low)

`is_token_char` is byte-for-byte identical in `header.rs:60-79` and
`method.rs:59-78`. Conventions rule: "Do not duplicate logic across files.
Fix the existing source instead." One private shared item fixes it; the
conventions explicitly bless a small shared module for ubiquitous helpers.

### C2 — `pub use url;` is scope creep on the hardest-to-change surface (medium-low)

`lib.rs:44-48` re-exports the whole `url` crate with a justification that
doesn't hold: consumers depending on `net` do not thereby avoid declaring
their own `url` dependency — Cargo unifies compatible semver requirements
regardless of re-exports. What the re-export does do is widen the public API
past ticket 11's "url::Url at the entry," in tension with decision 01's
public-surface-is-sacred principle. Conventions: import from the defining
module; no barrel re-exports. Cut it while cutting it is free.

### C3 — Every `TestServer` teardown manufactures a spurious handler panic (medium-low)

Observed live during the probe run: `Drop` (`common/mod.rs:272-281`) dials
one probe connection to unblock `accept()`, the accept loop hands it to the
real test handler, and the handler panics ("peer closed mid-request-head")
into the failures list. Nothing catches fire today only because every test
happens to call `assert_clean()` before dropping the server — an ordering
invariant written down nowhere. One test that asserts cleanliness after
teardown-related logic, or a handler that outlives its welcome
(`await_peer_close` adds up to 2s per teardown), and this becomes a mystery
flake. Route the probe connection around the handler (flag checked before
dispatch, or a dedicated shutdown socket).

Relatedly, `Err(_) => break` in the accept loop makes EMFILE-style failures
silently kill the server instead of failing loudly.

### C4 — Error-mapping seam has zero tests, and one mapping lies by design (low)

Ticket 10 names "focused units for error mapping" as testing priority 4;
none exist. Most `From<ureq::Error>` arms are constructible and cheap to pin
in a table-driven unit test — this is the seam the whole crate leans on.
Two specific nits inside it:

- `error.rs:172-177` maps unknown future timeout variants to `RecvBody`, so
  a hypothetical new knob reports "recv-body timeout exceeded" — a
  fabricated diagnostic; carry the raw name instead.
- `U::BodyExceedsLimit(usize)` throws away the limit number that would have
  explained the failure.

### Minor notes

- `HeaderMap::insert` (`header.rs:98-113`) accepts leading/trailing SP in
  values (lenient vs strict RFC 9110 `field-content`). Tolerable, but it
  means a value our boundary accepts can still bounce off the backend at
  `send()` time as `NetError::Protocol` — two different rejection latencies
  for the same class of garbage. Decide which layer owns value grammar.
- `Response` docs mention the lowercase caveat but not the
  unordered-iteration caveat that ticket 02 records for received headers;
  `header.rs` module docs cover it. Keep the story in one place.
- Size discipline: ticket 11 doesn't demand it, but this diff pulls ureq +
  url (+603 lockfile lines) into the binary's dependency graph, and repo law
  is "measure size at milestones." When tickets 08 + 12 complete M1, the
  ≈+700 KB marginal goes in `wiki/researches/size-budget.md` — don't let the milestone
  slip past the measurement.

## Verdict

Solid work overall: the seam discipline is real, the streaming / drop-cancel
tests are genuinely adversarial (the incremental-streaming deadlock trap is
better than most engines' tests), and amending ticket 02 when the wire
disagreed with it is exactly right. Not clean, though:

- Fix before merge: **S1** (wrong wire behavior behind a false spec
  citation), **S2** (unrecorded contract shrink), **C3** (planted flake).
- Cheap follow-up: S3, S4, C1, C2, C4, minors.

## Round 2 — test-suite audit (2026-08-26)

Question: do the tests verify, can they be trimmed? Findings, all fixed in
this round:

1. **UA stacking relied on backend mercy** (medium): `send()` appended the
   agent UA plus user headers unconditionally; only ureq's suppression kept
   duplicates off the wire. The layering is now decided in `send()`
   (explicit request-level `User-Agent` overrides the agent default; one UA
   ever) and pinned by `request_level_user_agent_overrides_the_builder`.
   The old test's "not stacked" comment claimed coverage it did not have.
2. **`final_url` never verified under an actual redirect** (medium): added
   `redirect_chain_updates_final_url_and_records_every_hop` (302→200 over
   two connections: status, final_url, body, hop sequence) and
   `redirect_cap_fires_as_limit_redirect_and_bounds_the_loop` (looping
   Location, cap 2 → `Limit(Redirect)`, hops bounded).
3. **Parser negative paths were dark** (low-medium): new
   `tests/token_grammar.rs` pins InvalidName rejections (empty/SP/NUL/DQUOTE/
   non-ASCII/LF injection), full-tchar acceptance, value grammar edges
   (HTAB + obs-text legal, NUL/CTL/DEL/LF/CRLF rejected, no partial state),
   method-token rejections, and equality-follows-wire-token semantics.
4. **Trimmed per ticket 10's own rules**: deleted
   `relative_urls_never_become_requests` (tested upstream `url::Url`
   parsing — type-guaranteed) and `display_prefixes_stay_stable_for_
   caller_matching` (pinned Display cosmetics nothing matches on).
5. **Message-text coupling removed** (low): verbatim-case method refusal
   now asserts the `Protocol` arm only, not ureq-proto's wording.
6. **Timeout + chunked gaps closed** (optional tier):
   `global_timeout_fires_when_the_head_never_arrives` (stalling server,
   120 ms budget → `Transport(Timeout(Global))`, confirming knob-based
   classification end to end) and
   `chunked_transfer_decodes_to_a_clean_body_stream`.

Suite after: 29 tests (18 loopback, 6→5 error-mapping, 5 grammar,
1 property), all offline, ~0.8 s total, clippy clean.

Not acted on: `context_survives_the_round_trip` kept as-is (it verifies
real `send()`→`from_backend` wiring, not getters); property strategy left
at a narrow name charset since the concrete tchar alphabet is now pinned
in token_grammar.
