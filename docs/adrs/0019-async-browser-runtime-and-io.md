# Async browser runtime and I/O

The browser process uses one Tokio runtime for protocol adapters, browser state,
tab coordination, networking, and renderer communication. Public browser
handles are async-only. The page engine stays synchronous and single-threaded
inside each renderer process.

Status: accepted (2026-09-14). This ADR defines the v2 migration. It supersedes
the blocking transport in [ADR 0006](0006-net-transport.md), the blocking
browser-side scheduling rules in [ADR 0010](0010-page-actor-ownership.md), the
private adapter runtimes in
[ADR 0012](0012-host-protocol-and-cli-stack.md), and the newline-delimited IPC
framing in [ADR 0016](0016-renderer-seam-reference-monitor.md). The crate
boundaries, process isolation goals, browser-side authorization, requirement for
finite resource bounds, and web-platform rules in those ADRs remain. V2 may
replace their specific mechanisms and limit values.

## Decision

### Runtime ownership

- The executable creates one multi-thread Tokio runtime for each browser
  process. The runtime runs one protocol adapter, the browser task, tab
  coordinators, network work, renderer channel tasks, and the top-level shutdown
  sequence.
- CDP and WebDriver expose async server functions. Neither adapter creates a
  runtime or moves browser calls through `spawn_blocking` or `block_in_place`.
- `BrowserHandle` and `TabHandle` are async-only value handles. V2 has no
  blocking facade.
- One browser task owns the tab registry, renderer process manager, profile
  state, and shutdown state. A bounded command channel connects
  `BrowserHandle` to this task. Browser state is not kept behind a mutex that a
  caller can hold across an await.
- One Tokio task coordinates each tab. The task owns the active navigation,
  navigation epoch, waiters, subscribers, site instance, and renderer handle.
  The task waits for commands, renderer events, network completions, exact
  deadlines, and cancellation. It does not poll and does not own an OS thread.
- One renderer process owns one current-thread Tokio runtime and one page
  engine. Under process reuse, the engine may host more than one top-level
  renderer assignment. The runtime waits for renderer channel I/O,
  browser-service replies, task deadlines, and shutdown. The page engine runs
  each selected HTML task synchronously to completion. `Dom` and QuickJS never
  cross an await or an OS thread.

The browser process may enable Tokio's networking, process, synchronization,
I/O utility, timer, macro, and multi-thread runtime features. The renderer may
enable only the Tokio features needed by its current-thread waiter, channels,
timers, and platform channel. The renderer does not use a multi-thread runtime,
an HTTP client, or a web-server stack. The workspace does not enable Tokio's
`full` feature or add another async runtime.

### Network transport

- `net` uses `hyper-util` for connection pooling, HTTP/1.1, and HTTP/2. It uses
  `hyper-rustls` with native roots and the `ring` crypto provider. Default
  `hyper-rustls` features stay disabled so AWS-LC is not selected by accident.
- Tinybrowser does not implement HTTP/1.1 or HTTP/2 framing. The `net` crate
  continues to own redirects, cookies, SameSite decisions, headers, timeouts,
  proxy policy, body limits, and public error types.
- The TLS connector stays behind a private transport seam. The stealth
  milestone can replace rustls with a `btls` memory-BIO connector without
  changing browser callers or public `net` types.
- Hyper-util's connector support provides HTTP `CONNECT`, SOCKS4, and SOCKS5.
  A custom resolver preserves ordered `--resolve` rules. Unmatched names use
  bounded system resolution. Happy Eyeballs stays enabled.
- Page WebSockets use Tokio and `tokio-tungstenite`. The browser process does
  not keep a blocking WebSocket thread per connection.
- Every request has an ID, an absolute deadline, an initiator kind and URL, a
  priority, an owned cancellation path, and a strict connection-pool key. The
  first pool key includes the origin, proxy configuration, TLS configuration,
  and privacy partition. HTTP/2 cross-origin coalescing stays disabled until
  the required security and privacy rules exist.
- Network limits are explicit and finite. Separate limits cover browser-wide
  requests, requests per tab, HTTP/1.1 connections per host, and WebSockets.
  Top-level navigation has priority over background resource work. The measured
  P7 checkpoint records the initial values.

Replacing a navigation cancels its previous request. Closing a tab cancels all
tab-owned requests. Renderer death cancels that renderer's service requests.
Cancellation removes pending replies, drops response bodies, and releases
connection permits. Redirects consume the original absolute deadline.

Typed DNS, connection, TLS, timeout, body-limit, and cancellation failures cross
the browser and renderer protocols. CDP and WebDriver map those failures only at
their protocol edges.

### Renderer platform channel

Each renderer uses one private, inherited, full-duplex platform channel.

- Unix and macOS use an unnamed `AF_UNIX` `SOCK_STREAM` pair created before
  spawn. The child endpoint is inherited without a public socket path.
- Windows uses an overlapped duplex named pipe. Windows support is not claimed
  until the named-pipe bootstrap and compile gate pass.
- Stderr stays on a separate pipe for diagnostics. The browser drains it with
  an async task. Renderer protocol traffic does not use stdout.
- `RendererChannel` hides platform bootstrap details from renderer process
  management and protocol routing.

The Unix child endpoint may occupy file descriptor 0. The child duplicates the
owned descriptor through safe standard-library APIs, enables nonblocking mode,
and registers it as a Tokio Unix stream. The design contains no hand-written
`unsafe` code.

One async reader task and one async writer task own each browser-side channel.
Bounded queues connect those tasks to the renderer process manager. Request IDs
map to oneshot replies. EOF, malformed input, writer failure, and child exit
fail all pending replies and begin renderer teardown.

The renderer side uses async channel I/O on the same current-thread runtime as
the page engine. A selected page task still runs synchronously. Slow page work
can delay that renderer, but it cannot block the browser runtime or another
renderer.

Synchronous browser-service calls are the one exception. `document.cookie` is a
synchronous JavaScript API that must complete a browser round trip while the
page engine runs, and a current-thread runtime cannot drive that reply while the
engine thread is blocked. The renderer therefore keeps one blocking reader and
one blocking writer thread for the channel. The renderer loop, its timers, task
deadlines, and shutdown all wait asynchronously; only the two transport threads
remain, and they carry no scheduler or polling work. Making the cookie service
asynchronous would remove them.

### Framing and body streaming

Renderer traffic uses length-prefixed frames. A fixed header carries the
protocol version, message kind, request ID, and payload length. The reader
validates the header and length before allocating the payload. A version
handshake completes before the browser mounts content.

Small control payloads use JSON. Response body frames carry raw bytes. Control
payloads retain the 8 MiB hard limit from ADR 0016. Each body chunk is at most
64 KiB. Aggregate response limits belong to request policy, not IPC framing.
Transport queues remain bounded by message count and retained bytes in both
directions. Unknown versions, unknown message kinds, invalid lengths, truncated
frames, and invalid control JSON fail closed.

A streamed response has this order:

1. `ResponseStart` carries the request ID, final URL, status, and headers.
2. Zero or more `BodyChunk` frames carry bounded raw bytes.
3. `ResponseEnd` or `ResponseError` terminates the response.

The first transport checkpoint may collect chunks in the renderer before it
mounts the document. A later checkpoint feeds chunks into encoding sniffing and
html5ever as they arrive. Parser-blocking scripts, `document.write()`, and
encoding behavior follow the HTML Standard. Focused WPT runs prove that later
web-platform change. Cargo tests cover framing, limits, cancellation, and other
tinybrowser-owned behavior.

### Renderer process policy

The renderer process manager owns process creation, handshakes, site locks,
assignment, failure, and reaping. Each mounted top-level document receives a
browser-minted `RendererAssignmentId` bound to its tab, navigation epoch,
principal, and renderer process. Commands, events, browser-service calls, and
cancellation carry this ID. The browser rejects stale IDs and IDs received from
the wrong renderer channel.

The manager applies these policies:

- Initial blank documents and opaque documents with no inherited site may share
  one unlocked renderer. The manager resolves inherited or precursor origins
  before assignment, so an `about:blank` or `blob:` document associated with a
  site uses a renderer locked to that site.
- An unlocked renderer may not gain a site lock while it hosts other documents.
  A site-backed navigation moves to a spare or new renderer. When the committing
  document is the unlocked renderer's only assignment, the manager may bind that
  renderer's immutable site lock.
- The manager keeps one unlocked spare renderer when memory permits.
- A soft process limit is derived from available memory. Under the limit, each
  live site instance receives its own renderer process.
- Over the limit, a site instance may reuse a renderer already locked to the
  same site. A renderer never hosts content outside its site lock.
- Memory pressure removes spares and idle processes before it changes live
  assignments.

Cross-tab sharing is a process-budget policy, not the default isolation rule.
Out-of-process iframes, renderer sandboxing, shared-memory IPC, and a separate
network process remain later work.

### Shutdown

`BrowserHandle::close().await` is authoritative and returns persistence and
teardown errors. Shutdown runs in this order:

1. Protocol adapters and the browser task refuse new work.
2. The browser cancels tab requests and closes every tab coordinator.
3. The renderer process manager closes channels, stops children, and reaps them.
4. Network tasks release bodies, permits, and pooled resources.
5. `ProfileStore` persists the quiescent cookie jar.
6. The executable stops the protocol listener and runtime.

Repeating `close` retries a failed durable profile write. `Drop` may request
best-effort cancellation. `Drop` does not write profile data, wait for child
processes, join threads, or hide an operation that can fail.

## Why

The v1 browser shell used one OS thread per tab, a fixed blocking network pool,
two renderer pump threads plus stderr forwarding, private runtimes in both
protocol adapters, and 10 or 20 ms polling loops. A 100-tab measurement reached
629 threads and 233 MB proportional set size. Idle CPU stayed low, but the
thread count and memory shape do not support the target scale.

Async I/O lets one browser runtime wait for many tabs, sockets, children, and
deadlines. It also gives cancellation a real ownership path. Keeping the page
engine synchronous preserves the single-owner `Dom` and QuickJS design.

Hyper is already present through axum. Hyper-util provides the HTTP versions,
pooling, proxy connectors, resolver seam, and readiness behavior that a browser
shell needs. A maintained protocol implementation is smaller in engineering
risk than a custom HTTP stack. A release probe measured the tuned
HTTP-only binary at 1,011,872 bytes and the HTTP/2-capable rustls binary at
2,023,832 bytes. The isolated TLS delta was 1,011,960 bytes. The integrated
checkpoint remains the authoritative size result.

A private platform channel preserves inherited-handle bootstrap and EOF
lifecycle without a public rendezvous address. Length framing bounds allocation
before deserialization. Raw body frames remove JSON byte-array expansion and
prepare navigation for incremental parsing.

The platform-channel shape follows mature browser implementations. Chromium's
[`PlatformChannel`](https://raw.githubusercontent.com/chromium/chromium/main/mojo/public/cpp/platform/platform_channel.h)
selects Windows pipes, Unix-domain sockets, or Mach ports and passes one endpoint
to a child. Chromium's
[POSIX Mojo channel](https://raw.githubusercontent.com/chromium/chromium/main/mojo/core/channel_posix.cc)
uses readiness notifications and queues partial writes. Firefox creates a
[nonblocking Unix stream socket pair](https://raw.githubusercontent.com/mozilla-firefox/firefox/master/ipc/chromium/src/chrome/common/ipc_channel_posix.cc)
on POSIX and an
[overlapped duplex named pipe](https://raw.githubusercontent.com/mozilla-firefox/firefox/master/ipc/chromium/src/chrome/common/ipc_channel_win.cc)
on Windows.

## Compatibility gates

- Cargo tests and Clippy cover tinybrowser-owned APIs, lifecycle, framing,
  cancellation, limits, and transport behavior.
- Playwright covers the external client acceptance surface.
- The promoted Blink inspector-protocol corpus covers CDP responses and events.
- Focused WPT runs cover each web-visible parsing, fetch, cookie, or WebSocket
  change. Cargo tests do not duplicate web-platform conformance cases.
- Dependency checkpoints record the stripped release binary size.
- Runtime checkpoints record navigation latency, a 50-request load, 100-tab
  creation, thread count, file-descriptor count, proportional set size, and idle
  CPU.

## Consequences

- This migration is a breaking API change. Callers await every browser and tab
  operation.
- Ureq and native-tls leave the active transport. The blocking network executor,
  per-tab browser threads, adapter runtimes, polling loops, and renderer pump
  threads leave with them.
- The renderer adds Tokio channel and asynchronous I/O features but no HTTP
  stack and no multi-thread runtime.
- Linux is the first implementation platform. The selected Windows transport is
  a separate named-pipe checkpoint before Windows support is claimed.
- Every dependency checkpoint records the stripped release binary size in
  [the size budget](../researches/size-budget.md). The x86_64 ceiling remains
  10 MB.
- The complete migration stays on `refactor/v2-async-browser`. Each checkpoint
  is committed only after its acceptance gates pass. One pull request covers
  the completed migration.

## Options considered

- **Keep blocking handles around async internals.** Rejected. A blocking facade
  recreates runtime nesting and makes cancellation and shutdown ambiguous.
- **Keep one tab thread and only replace networking.** Rejected. The browser
  would retain its largest thread multiplier and polling coordinator.
- **Run the page engine as freely scheduled futures.** Rejected. DOM and
  QuickJS ownership stays on one renderer thread, and each page task runs to
  completion.
- **Write an HTTP/1.1 and HTTP/2 client.** Rejected. Hyper-util already supplies
  maintained protocol implementations, pooling, proxies, and resolver hooks.
- **Keep ureq behind `spawn_blocking`.** Rejected. Running requests cannot be
  cancelled reliably, and slow hosts still consume a bounded worker lane.
- **Use loopback TCP for renderer IPC.** Rejected. Child renderers do not need a
  public rendezvous address, authentication token, or port allocation.
- **Keep two standard-I/O pipes.** Rejected. Renderer stdin uses a blocking
  helper thread in Tokio and cannot cancel an ordinary read. One inherited
  platform channel is async on both sides.
- **Add a binary serializer or shared memory now.** Rejected. JSON remains linked
  for the protocol adapters, and raw bounded body frames remove the main payload
  cost. Shared memory adds synchronization and crash-cleanup work before the
  measured channel needs it.
- **Use one renderer process per site instance without a budget.** Rejected. The
  default isolation shape remains, but 100-tab operation needs a soft process
  limit, blank sharing, and same-site reuse under pressure.
