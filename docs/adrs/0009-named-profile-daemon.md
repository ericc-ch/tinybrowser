# Named profile daemon and local CDP

tinybrowser ships one executable. There is no separately shipped or versioned helper such as chromedriver or Node. A CLI command starts a detached background process of that same executable when the selected profile daemon is missing. Control for CLI and external tools is CDP. Classic WebDriver is a peer adapter over the same browser, not a second owner.

Status: accepted. Keeps [ADR 0007](0007-engine-charter.md) crate graph, Tokio `rt`+`time`, bounded blocking-network execution, and size/lint bounds. The `cdp` adapter is a peer crate that depends on `browser` and inbound `axum` (retired `http1` by [ADR 0012](0012-host-protocol-and-cli-stack.md)). Extends [ADR 0008](0008-wpt-via-webdriver.md): WebDriver stays the WPT driver. Browser, `TabHandle`, and `NetworkSession` live in [ADR 0010](0010-page-actor-ownership.md).

## Process

There is one daemon per named profile. The implicit profile name is `default`. This daemon is the browser process for now. It runs one Browser bound to that Profile ([ADR 0010](0010-page-actor-ownership.md)).

The daemon binds only `127.0.0.1` on a random available port. It stops on explicit stop, OS user-session exit, or failure. It does not idle-exit. It never binds a remote address by default.

Registration is user-only at `$XDG_RUNTIME_DIR/tinybrowser/<profile>/daemon.json` and is atomically replaced. It records PID and endpoint data enough for health checks and stale-owner cleanup. A startup lock in the same profile directory lets concurrent CLI invocations elect one daemon. This does not protect against a hostile process running as the same user.

First-slice trust is Chrome-style local CDP: bind `127.0.0.1`, user-only runtime files, no `Host`/`Origin` checks, and no websocket token. There is no custom CDP authentication protocol.

The same executable may later self-spawn renderer workers. That is still one shipped program. The process seam is [ADR 0010](0010-page-actor-ownership.md).

## Durable data

Durable browser data lives at `$XDG_DATA_HOME/tinybrowser/profiles/<profile>/` (using the XDG default when the variable is unset). Opening a store requires `XDG_DATA_HOME` or `HOME`; missing both is an error. Cookie files are mode `0o600`. Cookies are first. Later site data uses the same store. Browser owns `ProfileStore`. CDP does not own cookies ([ADR 0010](0010-page-actor-ownership.md)).

## Control

CLI and external tools speak CDP, not a private RPC. Ship standard discovery and honest first subsets of Browser, Target, Page, and Runtime. Unsupported commands return method-not-found. Never fake support.

Support the browser WebSocket and direct page WebSockets. On the browser socket, `Target.attachToTarget` with `flatten` true returns a `sessionId`. Later target commands and events carry `sessionId` at the top JSON level. Direct page sockets need no `sessionId`. Internally both route to the same `TabHandle`.

CDP allows multiple client attachments. Tab actors serialize commands. Concurrent controllers can still interfere. Do not promise transactional isolation.

Initial CLI behavior creates, lists, selects, evaluates in, navigates, and closes targets through CDP. A last-target convenience is allowed. Target IDs stay explicit and visible.

Keep CDP off axum, hyper, and Tokio `full` ([ADR 0007](0007-engine-charter.md)). Prefer a small HTTP plus WebSocket server.

## WebDriver

Classic WebDriver is a peer adapter over `BrowserHandle` and the selected persistent profile. It does not own Browser, Tab, Profile, or NetworkSession. A WebDriver session owns only its current browsing context, timeouts, element references, and input state.

Allow one active classic WebDriver HTTP session, matching the endpoint-node model. Product `DELETE /session` detaches automation and leaves persistent tabs and profile data alive. Explicit close-window closes the selected tab, including the last tab, with spec-correct session behavior.

WPT isolation, runner teardown, and the future launch flag are in [ADR 0008](0008-wpt-via-webdriver.md).

## Options considered

- **Sidecar daemon binary:** a second program to ship and version. Rejected. One executable. A second OS process of this executable is required.
- **Idle shutdown:** surprises agents that pause between commands. Rejected.
- **Remote bind by default:** the local CDP trust model assumes loopback. Rejected.
- **Custom CDP auth in the first slice:** extra protocol before a working CLI. Rejected for v1. Loopback plus user-only runtime files is the Chrome-style local trust.
- **Private RPC for the CLI:** a second control surface beside CDP. Rejected.
- **WebDriver owns Browser and pages:** fights the persistent daemon. Rejected. WebDriver is an adapter.
- **Many classic WebDriver HTTP sessions:** endpoint-node browsers expose one. Rejected for classic HTTP.

## Consequences

- `tinybrowser --webdriver=PORT` is the WPT endpoint: classic WebDriver over `BrowserHandle`, with a runner-supplied temporary profile ([ADR 0008](0008-wpt-via-webdriver.md)). The product CLI talks CDP to a named-profile daemon.
- Cookies persist through `ProfileStore` under `XDG_DATA_HOME`. The live jar stays on `net::Agent`.
- The `cdp` crate depends on `browser` and inbound `axum` ([ADR 0012](0012-host-protocol-and-cli-stack.md)). Root wires it beside `webdriver`; the protocol crates do not depend on each other ([ADR 0007](0007-engine-charter.md)).
