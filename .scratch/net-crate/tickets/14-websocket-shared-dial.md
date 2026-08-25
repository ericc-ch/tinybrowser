# 14: WebSocket via shared dial path

What to build: One net-owned dial function (connector + TLS + proxy
config) used by both the HTTP backend and WebSocket upgrades; tungstenite
pump thread owning the socket after the 101; handshake through `Agent`
with `Context::WsHandshake`; the JS-visible handle (send / close /
take-next-message) with RFC 6455 §7.4 close codes surfaced. Closes M3 and
the effort's doc debt.

Blocked by: 12

Status: open

- [ ] ws:// loopback echo works through the public handle in both
      directions; pings answered automatically by the pump
- [ ] Fragmented text messages reassembled before delivery (recording
      server sends fragments; handle sees one message)
- [ ] Close handshake surfaces code/reason on the handle
- [ ] Code-review check: exactly one dial path — `send()` and the WS
      upgrade call the same function
- [ ] Marginal measured (+184 KB expected) and row recorded in
      docs/size-budget.md
- [ ] Docs debt cleared once implementation completes: CONTEXT.md gains
      **Hard seam**, **Context**, and **Conversion point** entries
      (deferred there by maintainer, 2026-08-25)
