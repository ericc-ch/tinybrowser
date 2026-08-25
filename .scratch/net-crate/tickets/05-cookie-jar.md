# 05: Cookie jar

Type: grilling

Question: Who owns cookie storage and enforcement, where does the jar sit
relative to the transport, and what does the public surface look like?

Answer:

- **Own jar in `net`, applied ABOVE the transport** (option a):
  `send()` harvests response `Set-Cookie` into the jar before returning,
  and builds the request `Cookie` header from the jar before dispatching.
  The backend runs with its cookies feature OFF and never knows cookies
  exist — so the btls swap cannot touch cookie behavior. This is the hard
  seam applied at the layer it matters most.
- **Why not the servo tiebreaker**: no servo jar exists; the ecosystem
  standard (`cookie_store`) is un-pluggable through ureq's public API and
  not Chrome-exact. The hand-rolled surface is small and exactly
  specified — RFC 6265bis §5.3 storage / §5.4 retrieval — and the stealth
  milestone forces a rewrite anyway (Chrome-exact serialization order:
  path-length then creation-time). Write once, now, where swaps can't
  reach.
- **Mechanics**:
  - Jar lives on `Agent` (`Arc<Mutex<…>>`), shared across clones alongside
    the pool/config.
  - SameSite enforcement keyed off request `Context` (decision 02);
    default `Lax` like modern Chrome — cross-site `Fetch`/`Xhr` contexts
    do not receive `SameSite=Lax` cookies; `Navigation` does.
  - Public surface for the future `document.cookie`: `cookies_for(uri)`
    and `set_cookie(set_cookie_value, uri)` on `Agent`. `browser` calls
    these; JS never sees the jar.
  - Storage/retrieval rules cite RFC 6265bis anchors inline per AGENTS.md.
- **Deferred**: disk persistence / serialization — lands with the CDP
  milestone (Network.setCookie et al need it); noted on the map.
