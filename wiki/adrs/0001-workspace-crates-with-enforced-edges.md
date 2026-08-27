# Workspace crates with enforced edges

The old monolith-and-prose-rules layout hid illegal imports from agents and from review. Crates exist so a `[dependencies]` line is a greppable build error.

Status: accepted for “not a monolith.” The live graph and which edges are law are in [ADR 0007](0007-engine-charter.md). This page keeps the history.

## Options considered

- **Monolith with drawn seams:** invisible when violated; only viable for a throwaway or an experienced reviewer.
- **Old seven-crate shape:** `cli`/`lib` were shells; `core` was bypassed, so `cdp` imported `dom`/`js`/`net` directly.
- **Four crates including empty `js`, root depending on all four, `js` must not import `net`:** the table this ADR originally shipped. Over-policing; superseded by 0007.
