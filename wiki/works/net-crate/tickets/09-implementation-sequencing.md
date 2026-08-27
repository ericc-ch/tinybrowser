# 09: Implementation sequencing

Type: grilling

Question: What order do net's milestones land in, what proves each one
done, and where do size numbers get recorded?

Answer:

- **M1 "dial"**: core types (ticket 02) + `url` crate (04) + ureq backend
  behind `send()`'s single conversion point + proxy/redirect knobs (08).
  Done when a real dial of example.com round-trips through our API.
  Expected marginal ≈ +700 KB tuned (490 + 197 + ε).
- **M2 "jar"**: RFC 6265bis storage/retrieval above the transport (05),
  SameSite keyed off `Context`, `document.cookie` methods. Done when a
  loopback server's Set-Cookie survives across two requests per spec
  rules. Near-zero binary cost (our own code).
- **M3 "ws"**: shared dial-path refactor (HTTP backend and WS call net's
  one dial function), tungstenite framing with caller-owned socket (06),
  handshake through `RequestBuilder::upgrade`. Done when loopback echo
  works through the public handle. Adds ≈ +184 KB (already probed).
- Ordering logic mirrors why option A was chosen: dials exist after M1,
  cookies unbreak real gates after M2, WS last because nothing waits on
  it. Every milestone records its marginal in [size-budget.md](../../../researches/size-budget.md) per repo
  discipline.
- **Out of this effort's scope**: `browser`/`js` wiring (HttpTransport
  impl, fetch bindings) — that is the follow-on effort once `net` stands
  alone; edges already fixed (tickets 03/07).
