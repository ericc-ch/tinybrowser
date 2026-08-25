# 13: Cookie jar above the transport

What to build: Our own RFC 6265bis cookie layer living above the transport:
harvest response `Set-Cookie` and build request `Cookie` headers inside
`send()` (backend cookies feature stays off), SameSite enforcement keyed
off request `Context`, and the browser-facing `cookies_for(uri)` /
`set_cookie(value, uri)` methods. Property-tested match rules. Parallel-
safe with ticket 12.

Blocked by: 11

Status: open

- [ ] Loopback two-request flow: server sets a cookie, next request
      carries it — covering domain-match, path-match, and host-only cases
- [ ] `Secure` cookies never sent over cleartext http; `HttpOnly` cookies
      invisible through `cookies_for()` (document.cookie semantics) while
      still riding request headers
- [ ] SameSite matrix: `Lax` cookie withheld from cross-site
      `Context::Fetch`/`Xhr`, delivered on `Context::Navigation`
- [ ] Expired cookies stop matching (proptest over timestamps)
- [ ] Proptest invariant: any stored cookie either returns for a matching
      URI or was correctly rejected by §5.3/§5.4 rules; RFC worked examples
      remain as concrete cases
- [ ] Binary delta ≈ zero confirmed at measurement (our code only, no new
      dependency)
