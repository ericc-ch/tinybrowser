# Renderer seam reference monitor and resource bounds

The browser process treats renderer messages as untrusted. Each renderer is
either unlocked for browser-assigned blank and opaque documents or bound to an
immutable schemeful-site lock. The browser validates principal-bearing service
calls against its own document assignment and renderer authorization, terminates
a process renderer on a violation, and bounds transport and event buffering.

Status: accepted and amended by
[ADR 0019](0019-async-browser-runtime-and-io.md). Hardens the process model in
[ADR 0011](0011-renderer-processes-per-site.md) without changing web-visible
fetch or cookie semantics. ADR 0019 replaces newline-delimited JSON with bounded
length-prefixed frames and raw response-body chunks.

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

- Renderer authorization has two states: `Unlocked` and `Locked(Site)`. A locked
  renderer authorizes HTTP(S) URLs only when their schemeful registrable domain
  equals its immutable `Site`. An unlocked renderer hosts only browser-assigned
  initial blank documents and truly opaque documents with no inherited site.
  The HTML Standard uses a site or tuple origin as an
  [agent-cluster key](https://html.spec.whatwg.org/multipage/webappapis.html#obtain-a-similar-origin-window-agent),
  while the browser process remains authoritative for the process assignment.
- The browser resolves inherited and precursor origins before process
  assignment. An `about:blank` or `blob:` document associated with a site uses a
  renderer locked to that site. `file:` remains separately privileged and never
  enters an unlocked renderer.
- The browser mints a `RendererAssignmentId` for each mounted top-level
  document. The assignment records its tab, navigation epoch, URL, principal,
  and renderer process. Commands, events, browser-service calls, and
  cancellation carry this ID. The browser rejects stale IDs and IDs received
  from the wrong renderer channel. The browser validates dial initiators and
  cookie URLs against both the assignment and the renderer authorization state.
  Invalid principals are protocol violations that close the channel, fail
  pending replies, and terminate the child. Logs never include
  renderer-provided URL or cookie text.
- A site-backed navigation does not bind a shared unlocked renderer. The process
  manager assigns the navigation to a spare or new renderer. The manager may
  bind an unlocked renderer only when the committing document is its sole
  assignment.
- The network adapter parses the authorized initiator once and passes the typed
  `Url` inward. Raw renderer text does not cross the authorization boundary.
- The inherited full-duplex platform channel uses length-prefixed frames.
  Readers validate the fixed header and payload length before allocation. JSON
  control payloads retain the 8 MiB message budget. Response body chunks use raw
  bytes and a 64 KiB cap. Aggregate response limits belong to request policy.
  Invalid, oversized, unknown, or truncated frames fail closed.
- Renderer transport queues have both message-count and retained-byte limits.
  Response-body delivery applies async backpressure. Saturated control and event
  queues fail closed so lifecycle messages are never silently dropped. Writer
  failures and failed browser-service replies terminate the renderer and release
  blocked callers. The platform-channel checkpoint records the initial
  capacities and retained-byte bounds.
- Tab command queues, retained waiters, event history, and subscriber
  registrations are finite. A slow protocol subscriber is disconnected instead
  of retaining unbounded browser memory. The tab-task checkpoint records the
  initial capacities.
- When a document leaves a tab, its work is cancelled and its process assignment
  is released. The process manager reaps the renderer, keeps it as the one spare,
  or reuses it only under the policy in ADR 0019.

The Fetch Standard still governs whether a response can be exposed to script:
cross-origin `fetch()` requires the
[CORS protocol](https://fetch.spec.whatwg.org/#cors-protocol). Process-lock
validation is an additional browser security invariant, not a substitute for
CORS, ORB, a renderer sandbox, or browser-owned cross-site frame routing.

## Consequences

- A locked renderer cannot use the browser's cookie API or forge a network
  initiator for a different site. An unlocked renderer is limited to its
  browser-assigned blank and opaque documents.
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
