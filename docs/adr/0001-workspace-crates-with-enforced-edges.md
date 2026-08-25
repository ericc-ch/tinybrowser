# ADR 0001: Workspace crates with enforced edges

Date: 2026-08-22
Status: Accepted (supersedes a same-day monolith decision)

## Context

The previous iteration (`tinybrowser-old`) used seven workspace crates
(dom, net, js, core, cdp, cli, lib). Two were thin shells (`lib`: 498 LOC,
`cli`: arg parsing), and the intended mediator (`core`) was bypassed: `cdp`
imported `dom`/`js`/`net` directly because nothing checked the manifests.

A monolith was briefly adopted here, argued from Node.js habit: one package,
folders plus prose rules, split later if needed. It was reversed for reasons
that would apply to any future re-review:

1. The codebase is built solo by a developer still learning Rust, with agents
   writing most of the code. Prose boundary rules ("don't import `dom` from
   `cdp`") are invisible when violated; manifest edges are build errors and
   greppable diffs.
2. The old repo proves drift happens even WITH crates once nobody checks them,
   so the edge rules can't rest on convention alone. (A cargo-metadata
   checker script was tried and cut as overkill for a five-manifest
   workspace; the working enforcement is that every edge is a visible
   `[dependencies]` diff.)
3. The seams are already known and proven stable across ~40k LOC; drawing
   them in cargo up front costs an hour, while splitting a 40k-line monolith
   later touches every import at once.

The Node analogy breaks down because Rust path-dependency workspaces carry no
version-management pain: no publishing, no lockfile juggling between members.

## Decision

Workspace crates + root package:

```
main → browser → {dom, net, js}
     (future: cdp → browser)
```

| crate | depends on | charter |
|---|---|---|
| `dom` | n/a | html5ever parse → arena tree, selectors. Sync, pure. |
| `net` | n/a | stealth HTTP/TLS client, cookie jar |
| `js` | dom | QuickJS embed + bindings; fetch injected via consumer-side trait, never depends on `net` |
| `browser` | dom, net, js | PageActor, navigation lifecycle; THE fan-in point |
| root `tinybrowser` | all four | embedder lib target + `serve`/`fetch` bins |

*Amended 2026-08-23 (`dom` and `browser` rows):* [ADR 0002](0002-dom-layer-architecture.md)
moved parsing out of the `dom` row (that crate is representation plus
selectors, Send-never-Sync, with name types from pinned markup5ever and no
parser dependency) and the TreeSink adapter landed in the `browser` row as
`browser::parse_html` ([ADR 0003](0003-treesink-adapter-in-browser.md)).
The edges themselves are unchanged.

Rules:

1. Dependencies go downward only; `browser` is the single fan-in point.
2. `unsafe_code` is denied workspace-wide with no pre-authorized home; if FFI
   work ever demands it, the allowance arrives with a written justification
   (see AGENTS.md Working rules). Panics must never unwind into QuickJS.
3. CDP is deferred; when it lands it is its own crate depending on `browser`
   alone, no-op stub domains included for Puppeteer/Playwright compat.

## Enforcement

No tooling beyond review: every edge is a `[dependencies]` entry in a member's
manifest, so violations surface as obvious diffs. The checker script was cut;
five manifests don't need a linter.

## Rejected alternatives

- **Monolith with drawn seams**: rejected per Context; viable only for a
  <5k-LOC throwaway or an experienced dev with review bandwidth.
- **Old seven-crate shape**: shell crates (`cli`, `lib`) fold into the root
  package's bin/lib targets; `core` is replaced by `browser`, which owns the
  page abstraction deeply enough that upper layers have no reason to bypass.
