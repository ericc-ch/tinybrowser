# Renderer processes and per-site isolation

tinybrowser isolates pages by **site instance**, not by tab and not by origin. Each
site instance runs its page engine in a **renderer process** — the same executable
spawned as `--renderer` — while the tab (`Page`), navigation, network, and cookies
stay in the host process. This replaces the "threads are the seam, process isolation
is later" posture of [ADR 0010](0010-page-actor-ownership.md).

Status: accepted. Replaces the Isolation section of [ADR 0010](0010-page-actor-ownership.md).
Extends the self-spawned-worker allowance of [ADR 0009](0009-named-profile-daemon.md).
`PageHandle`, `PageId`, and the protocol adapters do not change.

## Decision

- The security and memory boundary is a **site instance**: scheme plus registrable
  domain (eTLD+1), for example `https://example.co.uk`. Subdomains share a site
  because `document.domain` and cookies do; ports do not affect site identity.
  This is finer than a tab (cross-site frames must split) and coarser than an origin
  (same-site frames must be able to share a heap).
- A site instance is unique per (site, browsing context group), following Chrome's
  Principal Instance rule. v1 has no opener groups and no iframe documents, so in
  practice each tab's current site is one site instance. The registry key is the
  site instance, never the tab.
- One OS process per live site instance: the same executable, invoked as
  `tinybrowser --renderer`. The process boundary, not a thread, is the isolation
  property.
- The host owns **`Page`** (tab): `PageId`, navigation state, the document URL, and
  the site decision; the host's `Browser` owns the renderer registry. The renderer
  owns **`Document`**: `Dom`, QuickJS realm, active parser, HTML jobs, and host
  timers.
- Navigation is host-driven. The host dials, observes the final URL and headers,
  computes the site, and mounts the document in the renderer for that site. A
  cross-site navigation mounts the new document in a different renderer; `Page` and
  `PageId` survive.
- Host-to-renderer communication is value-only over IPC: commands, request ids,
  events, script results, and explicit errors. DOM handles, QuickJS values,
  callbacks, Rust borrows, and `net` types never cross.
- Renderer code never links `net`. Dials, cookies, and profile persistence are host
  services reached through the seam; the host's `NetworkSession` remains one per
  profile.
- One frame per tab in v1. Renderers are spawned per site instance so that
  same-site iframes can join a renderer and cross-site iframes can get their own
  (OOPIF) without moving owners.
- QuickJS 5 s / 32 MiB / 512 KiB stay per-realm v0 survival knobs, not web-platform
  numbers. Renderer count is host policy capped from available RAM later. There is no
  shared QuickJS heap.

## Why the site cut

- **Same heap is not isolation.** Threads in one process share an address space; a
  read primitive in one page reaches every other page in the process. Only separate
  address spaces make the boundary real.
- **Tabs are the wrong grain.** Once iframe documents exist, two sites in one tab
  must not share a heap, and one site's frames in a tab must share one (synchronous
  `document.domain` access, `window.opener`). Chrome locks each renderer process to
  documents of one site for exactly this reason, and Firefox's Fission remote type
  is `webIsolated=$SITE`.
- **Origins are the wrong grain.** `document.domain` lets sibling origins share
  synchronously, and cookies are scoped to a registrable domain. Splitting by origin
  breaks compatibility for no v1 security gain.

Engine ground truth:

- Chromium, [Process Model and Site Isolation](https://chromium.googlesource.com/chromium/src/+/main/docs/process_model_and_site_isolation.md):
  a site is "scheme plus eTLD+1"; a Principal Instance (a `SiteInstance`) is "the
  core unit of Chromium's process model"; "any two documents with the same principal
  in the same browsing context group must live in the same process" because they have
  synchronous access; cross-site frames become out-of-process iframes.
- Firefox, [Process Model](https://firefox-source-docs.mozilla.org/dom/ipc/process_model.html)
  and [BrowsingContext and WindowContext](https://firefox-source-docs.mozilla.org/dom/navigation/BrowsingContext.html):
  the `BrowsingContext` tree lives in the parent; content processes hold the
  `WindowGlobalChild` and the document; Fission uses `webIsolated=$SITE` pools,
  selected per navigation from the site and response headers by
  `ProcessIsolation.cpp`.

## Consequences

- The first navigation to a new site pays process startup. Preallocated renderers
  are a later optimization, not a v1 requirement.
- A renderer crash loses that document, not the browser process. The host reports it
  as a page error and can reload; v1 does not promise automatic recovery.
- The `browser` crate splits: a `renderer` crate owns the page engine and the
  renderer entry point; `browser` keeps host ownership. The seam's value types are
  defined before the crate split, so the crate boundary is not designed twice.
- Tests use an in-process renderer backend for speed where the process boundary is
  not under test; at least one loopback E2E test crosses a real `--renderer`
  process.
- Sandboxing (seccomp, namespaces) is a later security phase. Separate address
  spaces and value-only IPC are the v1 properties.
- `--renderer` is an internal mode, not a user feature, and is hidden from help.

## Options considered

- **Threads only (status quo):** keeps ownership and scheduling simple, gives no
  memory isolation and no crash isolation. Rejected; it was only ever a first cut
  of the seam.
- **Process per tab:** a cross-site frame either shares the tab's heap (no
  isolation) or cannot synchronously reach same-site siblings (compat break), and
  OOPIF cannot land later. Rejected.
- **Origin-per-process:** strongest cut, breaks `document.domain` and cookie site
  semantics, and multiplies processes for no v1 requirement. Rejected.
- **A separate renderer helper binary:** banned by the one-executable rule
  ([ADR 0009](0009-named-profile-daemon.md)). The same executable self-spawns.
- **Sandbox the renderer now:** deferred. The address-space boundary is worth having
  before the sandbox policy work begins.
