# Handoff (2026-09-10)

State: Per-site renderer-process split landed on top of the earlier dirty tree,
then hardened from two adversarial reviews. `cargo test --workspace` green
(three consecutive full runs), `cargo clippy --workspace --all-targets` and
`cargo fmt --all --check` clean. Nothing committed. Latest release measure:
CLI 4,632,736 bytes, page_probe 3,600,016, against the 10 MB cap.

Done:

- [ADR 0011](../docs/adrs/0011-renderer-processes-per-site.md): renderer process
  per site instance; tab `Page` in the host. [ADR 0012](../docs/adrs/0012-host-protocol-and-cli-stack.md):
  axum + clap, `http1` retired, 10 MB cap. ADR 0007 table, AGENTS.md, CONTEXT.md,
  size-budget.md updated.
- New `crates/renderer`: parser/`Sink`, `Document` (`Dom` + QuickJS + jobs +
  timers), value-only `protocol` (serde), `HostServices`, `--renderer` stdio
  transport. The crate has no `net` dependency (tests use it as a dev-dependency
  for a test host).
- `crates/browser` is host-only now: `Browser`, tab `Page` + `PageActor`
  coordinator, `RendererRegistry` with Local/Process backends, host-side
  navigation dials, cookies on the shared `NetworkSession`.
- CLI on clap; CDP and WebDriver on axum; `crates/http1` deleted.
- Tests moved: renderer document suite (9) + html5lib corpus; new cross-site
  document-swap test; new end-to-end `--renderer` process tests (classic script,
  JS `fetch`, non-finite results, close during a running script, opaque renderer
  reap).
- Two adversarial reviews landed; fixes: non-finite JS numbers are string-encoded
  in the IPC seam, a `Ready` handshake bounds bad renderer spawns, dead renderers
  drain pending requests (with a request timeout), opaque renderers are reaped on
  release, idle pools drain on Browser close, local dials run on the browser
  executor, and CDP discovery runs on `spawn_blocking` with trailing-slash
  normalization. `RemoteValue`/reply/command round-trip tests added.

Next:

1. Run `./tools/wpt/run` on the process path (not run this session).
2. Renderer sandboxing (namespaces/seccomp) — separate security phase.
3. Host-side navigation resolves relative URLs against the document URL only;
   `<base href>` is still honored inside the renderer for scripts/fetch. Add
   base-URL reporting if navigation needs it.
4. OOPIF when iframe documents land; renderer preallocation and a RAM-based
   renderer count cap.

Gotchas:

- `Browser::open_in` / `open_with_network` default to `Renderers::Process`;
  library tests must use `open_in_with(..., Renderers::Local)` or `ephemeral`,
  because the test binary cannot host `--renderer`.
- The renderer child speaks JSON lines on stdout; never `println!` in renderer
  code.
