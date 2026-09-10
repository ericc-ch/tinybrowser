# Vocabulary: browser process, renderer process, tab

tinybrowser's process, tab, and engine names drift between "host", "renderer",
"page", and "document". We align them with the browser-engine words that already
exist: Chromium's **browser process** / **renderer process**, Gecko's
**parent process** / **content process**, the HTML spec's **navigable** /
**top-level traversable** / **Document**, and CDP's **target** / **session**.

Status: accepted (2026-09-10). Supersedes the terminology of
[ADR 0009](0009-named-profile-daemon.md), [ADR 0010](0010-page-actor-ownership.md),
and [ADR 0011](0011-renderer-processes-per-site.md) where they say "host",
"page", "HTML job", or "context". The mechanisms in those ADRs are unchanged.

## Decision

### Processes

- **browser process**: the process that runs `--daemon` or `--webdriver`. It
  owns the UI-less equivalents of UI, tabs, profiles, cookies, and networking.
  Equivalent to Chromium's browser process and Gecko's parent process; those are
  documented synonyms, not names we use.
- **renderer process**: the process that runs `--renderer` and hosts the page
  engine for one site instance. Equivalent to Chromium's renderer process and
  Gecko's content process.
- "host" is retired as a noun. Use **browser process** for the process and
  **browser side** for the role ("the browser side of the seam"). Code renames
  `HostServices` to `BrowserServices`.

### Tabs

- **tab** is the canonical name for a top-level browsing context: the thing a
  user-visible tab or a CDP target of type `page` maps to. It is a spec
  **top-level traversable** and a WebDriver **window**.
- On the CDP wire this object stays a **page target**: `/json` and
  `Target.getTargets` report `"type": "page"`, and the `Page.*` methods are the
  protocol surface. CDP also defines an experimental **tab target**
  (`Target.createTarget { forTab: true }`) for the browser-UI tab container; we
  do not model it, so `Tab` in our code never means a CDP tab target. Chromium's
  content layer calls this object a `WebContents`, and Gecko's parent process
  calls its analog the `CanonicalBrowsingContext`.
- Code renames the `Page` family to `Tab`: `Page` → `Tab`, `PageId` → `TabId`,
  `PageHandle` → `TabHandle`, `PageActor` → `TabActor`, `PageError` →
  `TabError`, `PageEvent` → `TabEvent`, `create_page`/`close_page` →
  `create_tab`/`close_tab`, `pages()`/`page(id)` → `tabs()`/`tab(id)`.
- "page" stays only where a protocol or the web uses it: CDP `Page.*` method
  names, CDP target type `"page"`, and prose about documents fetched over HTTP.
  It never means the `Document` type and never means the `Tab` type by itself.

### Engine

- **page engine**: the code that owns a `Document` — HTML parsing, `Dom`,
  QuickJS, tasks, timers. It is the `renderer` crate. "renderer" alone always
  means the renderer *process*; qualify the process. Gecko's "content process"
  is a documented synonym.
- **Document**: unchanged, per DOM/HTML. Navigation replaces it; the tab
  survives.

### Isolation

- **site**: scheme plus registrable domain (eTLD+1). Code renames `SiteKey` to
  `Site`.
- **site instance**: one site within one browsing context group; the renderer
  registry key and the unit one renderer process serves. Matches Chromium's
  `SiteInstance`; Gecko's `webIsolated=$SITE` remote type is the analog.

### Scheduling and platform words

- **task** replaces "HTML job": one unit of page work the HTML event loop
  orders. The queue is the **task queue**. ECMAScript microtasks stay "jobs"
  (`execute_pending_job`), per ECMAScript.
- **initiator kind**: `net::Context` is the class of a request's initiator
  (`Navigation`, `Fetch`, `Xhr`, `WsHandshake`). Rename the type to
  `InitiatorKind` and the builder method to `with_initiator_kind`. Bare
  "context" always means browsing context.
- **platform object** replaces "host object": a JS object implementing a
  WebIDL interface, per WebIDL.
- `BrowserHandle`, `TabHandle`, and `RendererHandle` are **value-only command
  handles**; the suffix is the rule, not an abbreviation.
- Protocol words are used only in protocol context: CDP **target**,
  **session**, `sessionId`; WebDriver **window**, **browsing context**,
  **element**.

## Why

Engine ground truth:

- Chromium, [Multi-process Architecture](https://www.chromium.org/developers/design-documents/multi-process-architecture/):
  "the main process that runs the UI and manages renderer and other processes
  [is] the **browser process**"; "the processes that handle web content are
  called **renderer processes**".
- Chromium, [Process Model and Site Isolation](https://chromium.googlesource.com/chromium/src/+/main/docs/process_model_and_site_isolation.md):
  a `SiteInstance` is "the core unit of Chromium's process model"; "any two
  documents with the same principal in the same browsing context group must
  live in the same process".
- Firefox, [Process Model](https://firefox-source-docs.mozilla.org/dom/ipc/process_model.html):
  the **parent process** launches all children; **content processes** load web
  content, and the doc notes "renderer" is Chromium's name for the same thing.
- WHATWG HTML, [Infrastructure for sequences of documents](https://html.spec.whatwg.org/multipage/document-sequences.html):
  a tab is a **top-level traversable**; the spec now says "modern
  specifications should avoid using the browsing context concept in most
  cases", which is why we keep "context" reserved for the spec meaning.
- CDP [Target domain](https://chromedevtools.github.io/devtools-protocol/tot/Target/):
  `createTarget` "creates a new page", and a target has a `targetId`; a
  flattened `attachToTarget` returns a `sessionId`.

## Consequences

- One mechanical rename pass across `renderer`, `browser`, `net`, `cdp`,
  `webdriver`, the root crate, tests, and examples. No behavior change.
- `docs/CONTEXT.md`, `AGENTS.md`, and `docs/architecture.html` adopt the new
  words. Earlier ADRs had their identifiers and nouns updated to match
  (filenames unchanged, decisions unchanged); this ADR is the mapping for any
  older branch or commit.
- Future iframes fit the vocabulary without a new word: a child navigable runs
  in the renderer process for its site instance.
- "page" in future code requires a protocol reason; reviewers can flag it.
