# Handoff (2026-09-24)

Goal: Land the architectural plan from the whole-codebase review (report copied to the
vault: `projects/tinybrowser/architecture-review-2026-09-24.md`). Ownership first, then the
protocol, then the engine internals. The user approved all recommendations: one owner per
concept, typed invariants, pipe-backed payload handles with a shared-memory seam later, and
lease-ready origins.

Plan: nine units, each ending in an executable check.
U0 baseline → U1 assignment registry + RAII release → U2 request deadlines + recovery →
U3 acyclic cross-tab calls → U4 committed origins + authorization → U5 one `Connection` RPC +
payload handles → U6 conformance moves (parallel) → U7 DOM transition core → U8 inline boxes.

State: Branch `refactor/browser-cleanup`, tree clean at `b6f2669`. All gates green on every
commit: `tools/check`, `cargo test --workspace`, Playwright (9 passed, 1 skipped), WPT
`webstorage/` 49/54 = 90.7%, `cookies/` 25/88 = 28.4% (recorded 23.9%). Release binary
8,700,472 bytes stripped.

Done:

- **U0 baseline** (`80f11a1`): `tools/check` + `cargo test --workspace` green before changes.
- **U1 assignment registry** (`337282a`): one `AssignmentRegistry` + RAII `Assignment`
  replaces the contexts/subscribers/released maps and the explicit release path. Proof:
  `tests/renderer_process.rs::aborted_tab_releases_its_assignment` fails when the abort path
  leaks the assignment (red run recorded) and passes with the registry.
- **U2 request deadline recovery** (`487b5de`): every renderer request runs under
  `request_timeout()`; a miss interrupts the renderer; a failed request releases the
  assignment. Proof: `a_wedged_renderer_is_interrupted_and_the_tab_recovers` asserts the
  deadline elapsed (≥400 ms), the eval failed, and the next navigation mounts in a fresh
  process; red run fails when the release is disabled.
  `TINYBROWSER_RENDERER_REQUEST_TIMEOUT_MS` overrides the 60 s deadline for tests.
- **U3 acyclic cross-tab calls** (`8ff7721`): `TabRegistry` (shared tab table), per-tab
  delivery mailboxes, owner-side `WindowMessage`/`OpenerTab` protocol deleted, `close_tab`
  spawns shutdown. Proof: `post_message_to_a_busy_tab_does_not_block_the_sender` proves the
  receiver is wedged (a probe command stays unanswered), that `postMessage` returns in under
  2 s, and that the delivery reached the mailbox; red run blocks 4.2 s and fails.
- **U4 committed origins + authorization** (`b6f2669`): `Assignment.committed_origin`
  recorded at mount, `authorize_origin`, `RemoteSessionGet` origin check, `BroadcastPost`
  site check (previously none), `WindowMessage`/`WindowClose` relatedness gates. Proof:
  `window_messages_are_limited_to_related_tabs` (forged unrelated message is not queued;
  related delivery is); red run fails when the gate is disabled.
- **U5a storage transport safety** (`2d76416`): `MAX_CONTROL_BYTES` 8 → 24 MiB, the
  worst legal storage frame (setItem/getItem/storage event with double-escaped quota
  values). Proof: `legal_storage_values_do_not_kill_the_renderer` stores 2.5M quotes and
  reads them back; with the old cap the renderer dies. This is a stopgap; the payload plane
  below is the real fix.
- Review report: `~/Documents/obsidian/everything/projects/tinybrowser/architecture-review-2026-09-24.md`.

In flight: none.

Next:

1. **U5b payload plane (the real fix for the control-cap stopgap).** Replace the generic
   `Client`/`Server`/`Router`/`Upload`/`Responder`/`Notifier` stack with one framed
   connection: typed enums per direction, one pending-call table, cancel + deadlines in one
   place, and a `PayloadRef` for bulk bytes (storage values, dial bodies, messaging). Route
   bulk payloads through the existing chunk framing (`wire/channel.rs`, `exchange::Frame`)
   in both directions, including the browser→renderer reply and notice directions that have
   no chunk path today, so `MAX_CONTROL_BYTES` can drop back to a small bound. Keep the sync
   renderer adapter and the cancel-before-call negative cache. See review findings 2.1, 2.2,
   3.8, and the `exchange.rs` dead lanes.
2. **U6 conformance moves** (can land as small PRs any time): move spec assertions out of
   `crates/dom/tests/selectors.rs` and `crates/net/tests/net/send_loopback.rs` into WPT;
   register the `wdspec` executor; fix or delete `tools/intl/test262` (it drives deleted CLI
   commands); add a smoke check per runner named in `AGENTS.md`. The `send_loopback.rs`
   comment claiming WPT lacks 308 coverage is false — the vendored files loop over 308.
3. **U7 DOM transition core**: one `attach`/`detach_subtree`/`destroy_subtree` owning
   connection snapshots, lifecycle, mutation records, side tables, and reclamation; shadow
   parent link; custom-element reactions on the lifecycle queue. See review findings 4.1-4.8.
4. **U8 inline boxes + one intrinsic-sizing service**: `LayoutBox` with fragments, paint/
   geometry/decoration on one tree, Parley `calculate_content_widths`, cached intrinsic
   sizes. See review findings 5.1-5.8.

Decisions made (details in the commit messages):

- Release is RAII; the released-id set became a monotonic high-water mark; id 0 stays a
  violation.
- A renderer that misses its request deadline is interrupted; a failed request releases the
  assignment.
- The browser owner task never awaits a tab coordinator; `postMessage` enqueues on a bounded
  per-tab mailbox.
- The browser records the committed origin; frame-level storage/dial/cookie checks stay
  site-level because frames carry their own origins and frame origin isolation is
  renderer-side.
- Unrelated `postMessage`/`close` targets are ignored, not fatal.

Gotchas:

- Run everything through `nix develop --command`; `tools/check` enters the shell itself.
- The daemon's stderr is `Stdio::null()` in `tests/common/mod.rs`. To observe daemon-side
  behavior from a test, set `TINYBROWSER_LOG=debug` and read
  `<data>/tinybrowser/logs/default.log`.
- A CDP `Runtime.evaluate` wraps the source in `Promise.resolve((source))`, so
  `while(true){}` is a syntax error, not a block. Use `(() => { while(true){} })()` to wedge
  a renderer, and assert the wedge (a probe command must stay unanswered) before trusting a
  timing assertion.
- Clippy denies `single_match_else`, `similar_names`, `too_many_lines`, and unfulfilled
  `#[expect]`s. Extract helpers instead of re-adding an expect unless the function really is
  a dispatch table.
- WPT slices are slow (`cookies/` ~27 min); run them in the background and compare the
  `TOTAL` row against `docs/progress.md`.
- `docs/progress.md`: update only the latest binary size, the total, and scored groups.
