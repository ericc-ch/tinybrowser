# Workspace crates with enforced edges

The old monolith-and-prose-rules layout hid illegal imports from agents and from review. The workspace is crates with downward-only dependencies, `browser` as the single fan-in point, because a `[dependencies]` line is a greppable build error and the seams were already known.

Status: accepted

| crate | depends on | charter |
|---|---|---|
| `dom` | n/a | arena tree + selectors; Send, never Sync; names from pinned markup5ever; no parser |
| `net` | n/a | sync HTTP/TLS client and cookie jar (v1 transport: [ADR 0006](0006-net-transport.md)) |
| `js` | `dom` | QuickJS embed + bindings; fetch injected via a consumer trait, never depends on `net` |
| `browser` | `dom`, `net`, `js` | page lifecycle and `parse_html`; the only crate allowed to hold several layers at once |
| root `tinybrowser` | all four | embedder lib + `serve`/`fetch` bins |

CDP, when it lands, is its own crate depending on `browser` alone. `unsafe_code` is denied workspace-wide.

## Options considered

- **Monolith with drawn seams:** invisible when violated; only viable for a throwaway or an experienced reviewer.
- **Old seven-crate shape:** `cli`/`lib` were shells; `core` was bypassed, so `cdp` imported `dom`/`js`/`net` directly.
