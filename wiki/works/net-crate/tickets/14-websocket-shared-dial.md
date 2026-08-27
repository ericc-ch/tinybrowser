# 14: WebSocket via shared dial path

What to build: One net-owned dial function (connector + TLS + proxy
config) used by both the HTTP backend and WebSocket upgrades; tungstenite
framing with the caller owning the socket after the 101 (no background
pump); handshake through `Agent` with `Context::WsHandshake`; the
JS-visible handle (send / close / take-next-message) with RFC 6455 §7.4
close codes surfaced. Closes M3 and the effort's doc debt.

Blocked by: 12

Status: done (2026-08-26)

- [x] ws:// loopback echo works through the public handle in both
      directions; pings answered automatically while reading
- [x] Fragmented text messages reassembled before delivery (recording
      server sends fragments; handle sees one message)
- [x] Close handshake surfaces code/reason on the handle
- [x] Code-review check: exactly one dial path — `send()` and the WS
      upgrade call the same function
- [x] Marginal measured (+184 KB expected) and row recorded in
      wiki/researches/size-budget.md
- [x] Docs debt cleared once implementation completes: wiki/CONTEXT.md gains
      **Hard seam**, **Context**, and **Conversion point** entries
      (deferred there by maintainer, 2026-08-25)
- [x] No background pump: caller owns the socket (ticket 06 updated)

Run notes:

- Offline: `cargo test -p net --test websocket` (plus full `cargo test -p
  net` after the shared connector).
- Dial: `crates/net/src/dial.rs` `open()`; HTTP via `NetConnector::connect`,
  WS via `RequestBuilder::upgrade`.
- Size: tungstenite 0.26 (`handshake` only; 0.30 was the original probe).
  M3 probe +603 KB vs empty; +113 KB vs HTTPS-only (standalone tungstenite
  was +184 KB). rustc 1.98.0, tuned profile.
