# 06: Layer ownership

Type: interview

Question: Who owns the cookie jar, `document.cookie`, cookie persistence, and `:lang()` inputs (`lang`, `xml:lang`, `Content-Language`)?

Answer:

- **Jar** stays on `Agent` (one store for HTTP and the page). `cookies_for` / `set_cookie` stay as the non-HTTP jar API (6265bis §5.8.2). They are not the DOM.
- **`document.cookie`** is a `browser` host function that calls those methods. Do not name net APIs after the DOM.
- **Persistence:** in-memory on the `Agent` until an embedder/profile saves to disk. Not a CDP feature.
- **`lang` / `xml:lang`:** attributes on `Dom`. `:lang()` grows to read `xml:lang` in `dom` when we bother.
- **`Content-Language`:** `browser` keeps it on the document after navigation. `net` only returns the header. `:lang()` in `dom` may read a document-level default the page set. Never “lands with net.”

Current code is not a broken jar. The wrong part is the story: ADR 0002 and `state.rs` say language leftovers wait on `net`; Agent comments talk like `document.cookie` lives there; old tickets parked persistence on CDP.
