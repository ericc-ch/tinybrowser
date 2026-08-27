# 06: WebSocket shape

Type: grilling

Question: What reads and writes RFC 6455 frames after the 101 upgrade,
who owns the socket, and how does page JavaScript reach it?

Answer:

- **Framing: `tungstenite`, behind a measured size gate** (option a).
  Ecosystem standard, Autobahn-green; handles the correctness traps
  (UTF-8 validation across fragmented text messages, interleaved control
  frames, closing-handshake state machine). **Probed same-day:
  +184 KB tuned** (handshake feature only, no TLS — we dial ourselves);
  `url` probed at **+197 KB**; both together **+336 KB** (~53 KB shared
  deps). Rows recorded in [size-budget.md](../../../researches/size-budget.md); gate passed, adoption stands.
- **Who writes tungstenite**: snapview org on GitHub (2,384 stars, active
  — 0.30.0 shipped July 2026, MSRV 1.85); lead contributor daniel-abramov
  (~207 commits), with alexheretic and agalakhov. ~282M crates.io
  downloads.
- **One dial path owned by `net`**: connector + TLS + proxy config shared
  by the HTTP backend and WS upgrades. Rationale: whether ureq surrenders
  a live connection after `101 Switching Protocols` is undocumented;
  instead of betting on that escape hatch, WS dials its own connection
  through the same config. The btls swap replaces one dial function.
- **Handshake**: ordinary request through `Agent` / `RequestBuilder`
  tagged `Context::WsHandshake`; finish with `upgrade()`. Cookie, UA,
  and Origin share `prepare_outbound` with HTTP `send()`.
- **Threading**: caller owns the socket post-upgrade (no background
  pump). `send` / `close` / `take_next_message` block on the calling
  thread; pings are answered while reading. JS event-loop delivery stays
  in `browser`/`js`.
- **Injection** mirrors ticket 03: trait defined in `js`, implemented in
  `browser`; JS `open/message/error/close` events map onto the handle.
- **Close codes** surface as the `close` event's code/reason per
  RFC 6455 §7.4.
- **Handshake headers**: `Origin` from `with_initiator` on upgrade;
  agent `User-Agent` when configured. Bare upgrade omits `Origin`.
