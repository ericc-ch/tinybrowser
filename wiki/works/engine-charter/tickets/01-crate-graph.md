# 01: Crate graph

Type: interview

Question: What crates exist, who may import whom, and which edges are law?

Answer:

Simplest shape is three deep modules, not four crates plus slogans.

- **`dom`**: arena, `NodeId`, mutations, selectors. Depends on nothing in this workspace.
- **`net`**: dial, jar, WebSocket. Depends on nothing in this workspace. Public types stay ours so a later transport swap does not leak ureq. That is a `net` implementation detail, not a reason for other crates to exist.
- **`browser`**: parser sink, and later page lifecycle, JS embed, wiring. Depends on `dom` and `net`.
- **Root `tinybrowser`**: the binary/embedder. Depends on `browser` only. Re-export whatever embedders need from there.
- **No `js` crate** until QuickJS plus bindings is a module with a small public surface that something other than `browser` would import. An empty crate is a shallow module. The old "js must not depend on net" rule is dropped; when JS exists it may call `net` or not, decided then on simplicity, not theology.
- **One compile-error law:** a future `cdp` crate depends on `browser` alone. It does not import `dom`, `net`, or a later `js` crate. Tests of a crate (`cargo test -p dom`) may use that crate directly. That is not reach-around.

Dropped as commandments: downward-only purity for its own sake, root depending on all four, fan-in as a slogan while Cargo.toml did the opposite, `HttpTransport` as a constitutional requirement (separate ticket).
