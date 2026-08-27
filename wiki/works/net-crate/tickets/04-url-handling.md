# 04: URL handling

Type: grilling

Question: Which code parses, normalizes, and resolves URLs, what does
`net` accept, and where do relative resolution and future JS bindings live?

Answer:

- **Adopt servo's `url` crate** (the reference implementation of the
  WHATWG URL Standard). Standing tiebreaker, now recorded in the map
  notes: *prefer servo-grade spec implementations over hand-rolling when
  the binary budget allows* — same logic as the html5ever-over-html5gum
  decision. The WHATWG URL spec (parser, host parsing, UTS46/IDNA tables)
  is weeks of fiddly work to hand-roll and a fingerprint liability to get
  subtly wrong.
- **Gate kept but softened**: the tuned marginal gets measured at the
  implementation milestone (probe exercising real parses, per
  [size-budget.md](../../../researches/size-budget.md) discipline) and recorded there — adoption is not revoked
  unless the number lands absurdly high; maintainer prefers servo here.
- **Facts behind the fork**: Cargo.lock had no url/idna/
  percent-encoding crates; ureq 3 speaks `http::Uri` and does NOT pull
  rust-url, so this was a deliberate add either way. The fat part is
  idna's Unicode tables (~300–500 KB est., unverified until probed).
- **Division of labor**:
  - `net::request` accepts **absolute URLs only** (type: `url::Url`).
  - Relative→absolute resolution — including `<base href>` — happens in
    `browser`, where the document context lives.
  - Future `new URL()`/URLSearchParams JS bindings come via the same
    injection pattern as `HttpTransport`; `js` touches neither `net` nor
    any URL crate directly.
