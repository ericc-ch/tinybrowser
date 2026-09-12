# Renderer processes and per-site isolation

tinybrowser isolates pages by **site instance**, not by tab and not by origin. Each
site instance runs its page engine in a **renderer process** — the same executable
spawned as `--renderer` — while the tab (`Tab`), navigation, network, and cookies
stay in the browser process. This replaces the "threads are the seam, process isolation
is later" posture of [ADR 0010](0010-page-actor-ownership.md).

Status: accepted. Replaces the Isolation section of [ADR 0010](0010-page-actor-ownership.md).
Extends the self-spawned-worker allowance of [ADR 0009](0009-named-profile-daemon.md).
`TabHandle`, `TabId`, and the protocol adapters do not change.
Browser-side authorization and resource bounds on this seam are specified by
[ADR 0015](0015-renderer-seam-reference-monitor.md).

## Decision

- The address-space and crash boundary is a **site instance**: scheme plus registrable
  domain (eTLD+1), for example `https://example.co.uk`. Subdomains share a site
  because `document.domain` and cookies do; ports do not affect site identity.
  This is finer than a tab (cross-site frames must split) and coarser than an origin
  (same-site frames must be able to share a heap).
- A site instance is unique per (site, browsing context group), following Chrome's
  Principal Instance rule. v1 has no opener groups or network-backed cross-site
  iframe documents, so in practice each tab's current site is one site instance.
- One OS process per live site instance: the same executable, invoked as
  `tinybrowser --renderer`. The process boundary, not a thread, is the isolation
  property.
- The browser process owns **`Tab`** (tab): `TabId`, navigation state, the document URL, and
  the site decision; the browser process's `Browser` owns the renderer registry. The renderer
  owns **`Document`**: `Dom`, QuickJS realm, active parser, tasks, and browser
  timers.
- Navigation is browser-driven. The browser process dials, observes the final URL and headers,
  computes the site, and mounts the document in the renderer for that site. A
  cross-site navigation mounts the new document in a different renderer; `Tab` and
  `TabId` survive.
- Browser-to-renderer communication is value-only over IPC: commands, request ids,
  events, script results, and explicit errors. DOM handles, QuickJS values,
  callbacks, Rust borrows, and `net` types never cross.
- Renderer code never links `net`. Dials, cookies, and profile persistence are browser-side
  services reached through the seam; the browser process's `NetworkSession` remains one per
  profile.
- Child frames currently share their top-level renderer. Network-backed
  cross-site iframe navigation must not ship until the browser owns the frame
  tree and can assign out-of-process iframes (OOPIF).
- QuickJS 5 s / 32 MiB / 512 KiB are renderer-process survival knobs, not
  web-platform numbers. Realms hosted by one renderer share its QuickJS heap.
  Renderer-count policy remains browser-owned.

## Why the site cut

- **Same heap is not isolation.** Threads in one process share an address space; a
  read primitive in one document reaches every other document in the process. Only separate
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
- Renderers are not pooled after navigation. The old process is terminated so
  its page work cannot outlive its document and idle processes cannot grow with
  navigation history. A future pool needs a measured benefit, a hard capacity,
  and a proven reset-to-quiescence operation.
- A renderer crash loses that document, not the browser process. The browser process reports it
  as a tab error and can reload; v1 does not promise automatic recovery.
- The `browser` crate splits: a `renderer` crate owns the page engine and the
  renderer entry point; `browser` keeps browser-side ownership. The seam's value types are
  defined before the crate split, so the crate boundary is not designed twice.
- Tests use an in-process renderer backend for speed where the process boundary is
  not under test; at least one loopback E2E test crosses a real `--renderer`
  process.
- Sandboxing (seccomp, namespaces) is a later security phase. Separate address
  spaces, value-only IPC, and the browser-side reference monitor in ADR 0015 are
  the current properties; they do not make an unsandboxed child safe against
  arbitrary native code execution.
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
