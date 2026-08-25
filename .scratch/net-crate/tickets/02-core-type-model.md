# 02: Core type model

Type: grilling

Question: What are `net`'s public types and their contracts — Request /
Response / Body / Error — such that both consumers (navigation, injected
JS world) can be built against them unchanged when the stealth backend
lands?

Answer (full interface sketch + navigation/fetch callstacks reviewed
in-chat, 2026-08-25):

- **Streaming-first `Body`**: lazy chunk reader (`read_chunk`) with
  buffered conveniences (`bytes(limit)`, `text(limit)`). Buffered paths get
  explicit size limits. Rationale: XHR progress events and fetch streaming
  are chunk-shaped; buffered-only would force a consumer-visible breaking
  change at the backend swap.
- **Status is data, not error**: `send() -> Result<Response, NetError>`;
  every HTTP status 200..=599 arrives as `Ok`. Enabled natively by ureq's
  `http_status_as_error(false)` set once at agent construction. Matches
  WHATWG fetch (#http-network-or-cache-fetch): network errors reject,
  statuses resolve.
- **`HeaderMap` owned by `net`, case+order preserving**, ASCII-case-
  insensitive lookup (RFC 9110 §5.1). Documented v1 fidelity caveat: ureq
  lowercases received header names and `http::HeaderMap` iteration order is
  arbitrary, so *response* headers populate lowercase/unordered under the
  v1 backend; request headers keep verbatim case/order. Backend swap
  upgrades fidelity, signatures never move.
- **`NetError` taxonomy**: `Transport(dns|connect|tls|timeout|io)` /
  `Protocol` / `Limit(redirect|size)`. Nothing status-shaped lives here.
- **Config lives on `Agent`** (builder: timeouts global+per-call, redirect
  cap, buffer limits, user-agent). Per-request overrides deferred until a
  consumer demands them.
- **`Context` enum rides every request** (`Navigation | Fetch | Xhr |
  WsHandshake`). Two justifications: SameSite cookie decisions are a
  function of initiator context (Firefox staples LoadInfo +
  CookieJarSettings onto every load), and post-swap Sec-Fetch-* canonical
  headers differ by context. Slot costs nothing now; retrofitting would
  touch every consumer.
- **URL split**: `Response::final_url()` exposes the post-redirect
  location (Firefox's `URI` vs `originalURI` distinction); original URL
  stays on the caller's request.
- **No charset decoding in `net`**: raw bytes + Content-Type string go up;
  WHATWG encoding sniffing belongs to the parse pipeline. Firefox parallel:
  nsIChannel also carries raw `contentType`/`contentCharset` separately.
  Rejecting ureq's `charset` feature keeps encoding_rs out of the binary.
- **Cancellation = drop**: dropping `Response`/`Body` closes the
  connection (ureq already behaves this way). AbortController/XHR-abort
  reduce to this between chunks. Written down before consumers depend on
  accident.
- **Rejected, explicitly**: Gecko's per-load security-principal/CSP/
  tracking-classification/service-worker-interception/alt-data/performance-
  timing machinery — multi-process-browser concerns outside our budget.

Facts sourced live from docs.rs (ureq 3.4.0) during the decision;
Firefox ground truth from netwerk/base/nsIChannel.idl and
dom/fetch/FetchDriver.h.
