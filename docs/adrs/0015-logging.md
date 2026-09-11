# Logging: levels, stderr console, async batched file

tinybrowser gets a real logging system: **levels, a console sink, and an async
batched file sink**, configured once per process by `--log-level`. The model is
inspired by Effect's
[`Logger`](https://github.com/Effect-TS/effect/blob/main/packages/effect/src/Logger.ts)
and [`LogLevel`](https://github.com/Effect-TS/effect/blob/main/packages/effect/src/LogLevel.ts):
levels, a flat record, independent sinks, a minimum-level threshold, and a
batched file writer. The API is Rust-shaped, not an Effect port: one
process-global logger, macros, `FromStr`, no generic output types, services,
fibers, or layers.

Status: proposed (2026-09-12). Adds a leaf crate to the graph of
[ADR 0007](0007-engine-charter.md) and crosses the renderer seam of
[ADR 0011](0011-renderer-processes-per-site.md) with one stderr pipe. Does not
change any protocol, `TabHandle`, or the daemon contract of
[ADR 0009](0009-named-profile-daemon.md).

## Context

Today the only diagnostics are six `eprintln!` sites (fatal CLI errors and two
bad-IPC-message branches). There are no levels, no filtering, no structure, and
no file. A detached daemon runs with `Stdio::null()` for all three streams
(`src/daemon.rs`), so it is currently completely silent. Renderer children
inherit stderr (`crates/browser/src/link.rs`), which is null under the daemon.

`tracing 0.1` and `log 0.4` are already in the dependency graph transitively
(axum and html5ever/native-tls), but nothing in tinybrowser emits through
either.

## Decision

### 1. New leaf crate `logging`, std-only

`crates/logging` is added to the workspace. It has **no dependencies** and is
the only logging facade. Root, `browser`, and `renderer` depend on it; `cdp`
and `webdriver` adopt it where they have diagnostics, and `dom`/`net` do not
(yet). No cycles: `logging` is a leaf.

### 2. A small Rust logger, inspired by Effect

- `Level` is `Error, Warn, Info, Debug, Trace`, ordered most severe to least.
  No `Fatal`/`All`/`None` sentinels in v1. `FromStr` is case-insensitive and
  accepts `warning`; `Display` renders `ERROR`, `WARN`, ...
- `Logger` is a concrete value: `Logger::new(Config)` builds the sinks and
  starts the file writer; `enabled(Level)`, `log(Level, target, fmt::Arguments)`,
  `log_forwarded(line)`, `flush()`. There is no generic `Logger<Message, Output>`
  interface.
- `install(Logger)` places it in a process-global `OnceLock`; a second install
  is ignored. Macros `logging::{error,warn,info,debug,trace}!` take an optional
  `target:` and default to `module_path!()`.
- **Default (nothing installed) is a disabled no-op.** Library crates and
  tests stay silent unless the process entry point installs a logger. This is
  the Rust answer to Effect's `MinimumLogLevel = None`.
- Format arguments are evaluated only after the level check, so a disabled
  `debug!` costs one boolean load.

### 3. Console sink writes to stderr, one logfmt line per record

stdout is reserved in every mode: CLI commands print results there, and a
renderer child's stdout **is** the JSON IPC seam. Console records therefore go
to stderr. This is exactly what Effect's `LogToStderr` reference exists for
("route built-in logger output to stderr while keeping stdout reserved for
protocol messages or data output"). A later `--log-stdout` flag is additive;
v1 has no stdout sink.

Both sinks use one line format, logfmt in the shape of Effect's
`formatSimple`/`formatLogFmt`:

```text
timestamp=2026-09-12T14:03:21.123Z level=INFO process=daemon pid=5217 target=browser::tab message="created tab"
```

Values containing whitespace, `"`, or `=` are quoted and `"` is escaped, per
Effect's `textOnly`/`escapeDoubleQuotes` rules. `\n` and `\r` in a message are
escaped so **one record is always one line**; the renderer forwarding path
depends on that invariant. Timestamps are RFC 3339 UTC with milliseconds,
computed locally (day-from-civil), no `chrono`.

### 4. File sink is async, batched, bounded, and drops

- One dedicated `logging-file` OS thread per process that has a file sink. Its
  `SyncSender<Message>` is bounded (1024 records); `log` uses `try_send`, and a
  full queue increments an `AtomicU64` instead of blocking the caller. The next
  batch writes one `WARN ... dropped N records` line.
- The writer batches up to 256 records / 64 KiB per `write_all`, with a 250 ms
  window; no `fsync` (logs are diagnostics, not durable data). `flush()` writes
  the pending batch and acks.
- Rotation is one backup at 8 MiB (`<file>` → `<file>.1`), checked in the
  writer thread only.
- An open failure is non-fatal: the logger warns once on stderr and runs
  console-only.
- "Low priority" means best-effort, non-blocking, drop-under-pressure. OS
  scheduling priority is **not** set: `std` cannot express it and `libc`
  `setpriority` needs `unsafe`, which the workspace bans.

### 5. `--log-level` sets the minimum level; `--verbose` is an alias

- `--log-level=<error|warn|info|debug|trace>` is a global clap option,
  default `info`. Explicit values win.
- `--verbose` (long-only) is shorthand for `--log-level=debug`. There is no
  `-v` short flag, no counting, and no `-vv`/`-vvv`: `-v` is wired to
  `--version` via clap's `ArgAction::Version` (and clap's default `-V` is
  disabled to avoid two version flags).
- There is one threshold per process (Effect's `MinimumLogLevel`); the file
  sink is not more verbose than the console in v1.

### 6. Per-process sink and level wiring

| Process | Console | File | Level from |
| --- | --- | --- | --- |
| CLI command | stderr | no | its own `--log-level`/`--verbose` |
| `--daemon` | stderr (null when detached) | `<data_home>/tinybrowser/logs/<profile>.log` | its own flags, passed by the spawner |
| `--webdriver` | stderr | same per-profile file | its own flags |
| renderer child | stderr, forwarded | no | `TINYBROWSER_LOG` env from browser |

- `data_home` is `ProfileStore::data_home()`; logs sit next to the store, not
  inside the locked `profiles/<name>` directory. Unresolvable home → console
  only.
- The browser spawns renderers with `Stdio::piped()` stderr and a
  `renderer-{id}-stderr` pump thread that forwards each line with
  `log_forwarded`, so renderer records reach the daemon's console and file with
  `process=renderer`. This keeps **one file writer** for the daemon (no
  cross-process rotation races) and keeps `stdout` pure IPC.
- The renderer child receives `TINYBROWSER_LOG=<level>` from the browser;
  `main` reads it in `--renderer` mode. A detached daemon receives the same env
  from `daemon::spawn_detached`, so `tinybrowser --verbose create ...` starts a
  daemon at `Debug` if one is not already running. A live daemon keeps its
  startup level; changing it needs a future CDP method.
- In-process (local backend, `tab_probe`) renderer records use the same global
  logger directly; no pump, no `process=renderer` tag.

### 7. Shutdown

`main` becomes a thin wrapper that calls `logging::flush()` before returning
any `ExitCode`; the writer thread also drains on channel disconnect. A crash
can lose at most the current 250 ms batch.

## Implementation shape

- `crates/logging/src/lib.rs`: `Level`, `Config`, `Logger`, global install,
  macros.
- `crates/logging/src/format.rs`: logfmt rendering, quoting/escaping, RFC 3339
  millis.
- `crates/logging/src/file.rs`: bounded queue, writer thread, batching,
  rotation, drop counter.
- `src/main.rs`: `--log-level`/`--verbose`, `-v` version action, per-mode
  `Config`, per-profile log path, flush wrapper; fatal errors become
  `logging::error!` after install.
- `src/daemon.rs`: `spawn_detached` passes the current level to the child via
  `TINYBROWSER_LOG`; the daemon logs its bound port.
- `crates/browser/src/link.rs`: piped stderr + forwarding thread;
  `TINYBROWSER_LOG` for the child.
- `crates/*/Cargo.toml`, root `Cargo.toml`: workspace member and dependencies.

## Why

- **Effect's model fits because our logs are records, not spans.** A record is
  a level, a timestamp, a target, and a message; sinks are independent
  consumers. `tracing` would add a subscriber tree, span state, and formatting
  machinery we do not use, and the file batching would still be hand-rolled.
- **A file sink must never apply backpressure to a page.** The renderer thread
  owns `Dom` and QuickJS; a synchronous file write on a slow disk would stall
  script execution. The bounded queue plus drop counter isolates that.
- **stderr is the only safe console channel.** stdout is result data in the CLI
  and protocol JSON in the renderer. Putting logs there breaks both.
- **One writer per file.** Renderer file logging or a shared O_APPEND file
  would make rotation and interleaving racy; forwarding through the browser
  costs one pipe and one thread per renderer.

## Consequences

- Binary size measured 2026-09-12 against a rebuilt `main` baseline
  (`ccf16b8`): **+36,592 bytes CLI, +18,128 bytes `tab_probe`** tuned and
  stripped, recorded in `docs/researches/size-budget.md`.
- Root, `browser`, and `renderer` gain a `logging` dependency; library code can
  now emit diagnostics, but stays silent until `main` installs a logger.
- The daemon writes up to 8 MiB + 8 MiB per profile under
  `$XDG_DATA_HOME/tinybrowser/logs/`. No `logs` CLI command in v1; deleting the
  directory is safe while no daemon runs.
- A log level is not inheritable by an already-running daemon or by later CLI
  calls; document in `--help`.
- WPT is unaffected: test runs use a temporary `XDG_DATA_HOME`, and the
  console sink is stderr.
- The `log` crate facade (html5ever parse errors) and `tracing` (axum) remain
  unbridged. Bridging `log` into `logging` is a candidate follow-up for parser
  triage, not part of v1.

## Tests (cargo, tinybrowser-owned behavior only)

- Level parsing, ordering, and threshold filtering; `--log-level` parsing,
  explicit-value-wins over `--verbose`, and the `-v` version action.
- logfmt formatting: quoting, escaping, single-line invariant, known RFC 3339
  timestamps.
- File sink: records appear after `flush()`; batching writes whole lines;
  rotation renames at the cap; open failure falls back to console-only.
- Drop path: the bounded queue rejects instead of blocking when the receiver is
  stalled, the logger counts the drop, and the next record emits a `dropped N`
  warning.
- No web-platform behavior is asserted here; WPT stays the conformance gate.

## Options considered

- **`tracing` + `tracing-subscriber`:** already partially linked via axum. But
  the subscriber tree, span registry, and filters are unused weight for a
  record/log model, and `tracing-appender` batching would still need custom
  drop accounting. Rejected for v1.
- **`log` + `env_logger`/`fern`/`log4rs`:** `log` is already linked through
  html5ever. It is a global-macro facade with no async batching and no bounded
  drop policy; `fern`/`log4rs` add more code than the sink we need. Rejected.
- **JSON and pretty formats:** Effect ships `formatJson` and
  `consolePrettyTty`. One logfmt format is enough for v1; a `--log-format` knob
  is additive later.
- **Renderer logs as IPC messages:** structured records over the existing
  stdout channel would avoid the pipe, but an unbounded channel lets a log
  flood delay replies, and the protocol should not carry diagnostics.
  Rejected.
- **Renderer opens the log file directly:** no forwarding thread, but multiple
  writers and rotation races. Rejected.
- **Console on stdout:** semantically wrong in every mode (results, IPC, null
  stdio) and breaks CLI consumers. Rejected, per Effect's `LogToStderr`.
- **Spans/annotations (Effect's `fiberRef` context):** no fiber model here and
  no consumer yet; the macro surface can grow a `fields:` form without breaking
  call sites. Deferred.
