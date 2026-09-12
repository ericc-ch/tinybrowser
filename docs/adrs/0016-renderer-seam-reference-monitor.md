# Renderer seam reference monitor and resource bounds

The browser process treats renderer messages as untrusted. Every renderer is
bound to an immutable schemeful-site lock before it receives content. The
browser validates principal-bearing service calls against that lock, terminates
a process renderer on a violation, and bounds transport and event buffering.

Status: accepted. Hardens the process model in
[ADR 0011](0011-renderer-processes-per-site.md) without changing web-visible
fetch or cookie semantics.

## Problem

The process split was value-only but not authoritative. `DialRequest.initiator`
and the URLs in cookie service calls came from the renderer and were trusted by
the browser process. A compromised renderer could therefore claim another
site's principal when asking the browser-owned network and cookie services for
work. The newline JSON reader and event subscribers also had no capacity limit,
so a hostile or stalled peer could retain unbounded browser memory.

A process boundary is not a security design by itself. Chromium describes its
browser-side `ChildProcessSecurityPolicy` as the reference monitor for Site
Isolation and applies process locks before content is loaded; its jail checks
prevent a locked renderer from requesting another site's data
([process model and Site Isolation](https://chromium.googlesource.com/chromium/src/+/main/docs/process_model_and_site_isolation.md#process-locks),
[security directory](https://chromium.googlesource.com/chromium/src/+/main/content/browser/security/README.md)).
Firefox likewise validates principals received from content processes in
`ContentParent::ValidatePrincipal`
([ContentParent.h](https://searchfox.org/mozilla-central/source/dom/ipc/ContentParent.h)).

## Decision

- `Site` is the browser-owned renderer lock. HTTP(S) URLs are authorized only
  when their schemeful registrable domain equals the lock; per-tab opaque
  renderers authorize only `about:`, `blob:`, and `data:` document URLs, never
  the separately privileged `file:` scheme. The HTML Standard uses a
  site or tuple origin as an
  [agent-cluster key](https://html.spec.whatwg.org/multipage/webappapis.html#obtain-a-similar-origin-window-agent),
  while the browser process remains authoritative for the process assignment.
- The document-URL mutator may establish the site before the first mount. It
  cannot rewrite a live document's origin; navigation is the only path that
  selects and mounts a renderer for a new URL.
- Both the local test adapter and process adapter validate renderer dial
  initiators and cookie URLs. Local adapters fail the operation. A process
  adapter treats a failed check as a protocol violation, closes the transport,
  clears pending replies, and kills the child. It never logs attacker-provided
  URL or cookie text.
- The network adapter parses the authorized initiator once and passes the typed
  `Url` inward. Raw renderer text does not cross the authorization boundary.
- Newline-delimited JSON remains the transport because replacing it does not
  improve the trust model by itself. Encoding and decoding enforce an 8 MiB
  message budget while writing and before deserialization; missing delimiters
  and invalid JSON fail closed. The existing 1 MiB navigation-body limit
  remains independent.
- Renderer inboxes retain at most 256 messages and outboxes at most 4,096.
  Saturation terminates the renderer so lifecycle messages are never silently
  dropped. Writer failures and failed browser-service replies also terminate
  the renderer and release blocked callers.
- Tab command queues, retained waiters, and subscriber registrations are each
  capped at 256. A tab retains only its most recent 1,024 product events,
  production renderer event logs are drained after publication, pending
  document events fail closed at 2,048, the renderer-to-tab handoff fails
  closed at 4,096 queued events, and a protocol subscriber that cannot accept
  256 queued events is disconnected.
- Renderers are terminated when their document leaves the tab. The previous
  unbounded idle pool was removed because its processes kept driving old page
  work and grew with navigation history.

The Fetch Standard still governs whether a response can be exposed to script:
cross-origin `fetch()` requires the
[CORS protocol](https://fetch.spec.whatwg.org/#cors-protocol). Process-lock
validation is an additional browser security invariant, not a substitute for
CORS, ORB, a renderer sandbox, or browser-owned cross-site frame routing.

## Consequences

- A renderer cannot use the browser's cookie API or forge a network initiator
  for a different site. Same-site subdomains remain in one process lock, as the
  selected process model requires.
- Memory retained by each transport message, tab command queue, event history,
  and event subscriber now has an explicit upper bound.
- Oversized script results or IPC messages fail the renderer request instead of
  consuming memory until the process or browser is killed.
- Full site isolation still requires browser-owned frame routing and OOPIF for
  remote cross-site iframe documents. Native-code containment still requires a
  renderer sandbox. Neither property is claimed here.

## Options considered

- **Trust renderer principals because the executable is ours.** Rejected: the
  renderer parses and executes hostile web content, so compromise is the threat
  model the process split exists to contain.
- **Validate only cookie calls.** Rejected: a forged request initiator can also
  change SameSite and future `Sec-Fetch-*` decisions.
- **Return an empty result after a process violation.** Rejected: continuing a
  compromised child makes later authorization ambiguous. Mature browser IPC
  boundaries fail closed.
- **Adopt a binary serializer first.** Rejected: serialization format does not
  bound allocation or establish authority. The small codec guard fixes both
  directions without a new dependency or measurable architectural weight.
