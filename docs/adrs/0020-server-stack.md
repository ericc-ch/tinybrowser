# Protocol adapter server stack

The CDP and WebDriver adapters serve loopback HTTP and WebSocket traffic with
axum, as [ADR 0012](0012-host-protocol-and-cli-stack.md) selected. This ADR
records the P13 probe that re-measured that choice against a hyper-direct
server. The decision is to keep axum for v2.

Status: accepted (2026-09-15). Keeps the server-stack choice of
[ADR 0012](0012-host-protocol-and-cli-stack.md) with measured evidence. The
constraint stays: HTTP server crates remain in browser-process protocol
adapters and never enter the renderer or `net`.

## Decision

- Keep axum behind the existing adapter boundary. It already passes the
  protocol gates, and its measured cost is 550,432 bytes over a minimal
  hyper-direct server that does **not** yet implement the CDP surface.
- Do not hand-write HTTP/1.1 routing, `Upgrade` handling, or WebSocket
  framing for this migration. A hyper-direct adapter would have to re-prove
  every gate, and [ADR 0019](0019-async-browser-runtime-and-io.md) forbids
  hand-written HTTP stacks.
- Revisit only on a named trigger: the stripped binary approaches the
  10,000,000-byte cap with less than 300,000 bytes of headroom, or an adapter
  needs HTTP/2 on the debug socket. The measured fallback is a hyper-direct
  server plus `tokio-tungstenite` for the socket.

## Probe

A standalone probe crate, `tools/stackprobe`, served the same two shapes in two
binaries: `GET /json/version` plus a WebSocket echo endpoint. The axum binary
uses `axum::extract::ws`; the hyper binary uses `hyper::upgrade::on` and
`tokio_tungstenite`'s `WebSocketStream::from_raw_socket`, so both carry a
working upgrade path. Both were built with the workspace's rustc 1.98.0,
`--release`, `strip = true`, `lto = true`, `codegen-units = 1`, and
`panic = "abort"`.

| Probe binary | Stripped bytes |
| --- | ---: |
| axum + `axum::extract::ws` | 1,335,048 |
| hyper 1 + hyper-util + tokio-tungstenite | **784,616** |
| Delta | 550,432 |

The delta covers axum's router, extractor, and upgrade machinery beyond the
hyper/tower pieces both shapes share. It is 7.4% of the P12 shipping binary
(7,414,144 bytes) and 21% of the remaining headroom to the cap (2,585,856
bytes). That is real but not decisive: the probe's hyper side does not
implement CDP's dispatch, session routing, or event fan-out, so it does not
yet pass the same protocol gates. Replacing axum for a 0.55 MB saving would
spend migration time on adapter plumbing instead of the remaining v2 work,
and the measurement can be repeated cheaply if the size trigger fires.
