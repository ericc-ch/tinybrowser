# Binary size

Method: `nix develop -c cargo build --release` (the shipped artifact; there is
no post-link step), then `size`, `nm -C --print-size --size-sort`, and
`cargo bloat`. Any codegen or profile change is gated on a WPT parity run
(`tools/wpt/cli.py run --score dom/nodes/`) that must match the previous
pass/unexp/timeout/crash counts, and a release smoke via
`tools/wpt/cli.py run --binary <release binary>`.

## Ledger

| step | `cargo build --release` |
| --- | --- |
| start: stable, `panic = "abort"`, no build-std, `.eh_frame` stripped post-link | 12.19 MiB |
| funnel codegen (shared conversion funnels, one `with_reactions` core) | 12.07 MiB |
| nightly + `[unstable] build-std` (std at `opt-level = "z"`) | 11.86 MiB |
| `panic = "immediate-abort"` (interim) | 10.94 MiB |
| `force-unwind-tables=no`, `tools/release` deleted (interim, immediate-abort) | 9.79 MiB |
| current: `panic = "abort"` + synchronous panic hook for diagnostics | **10.61 MiB** |

`panic = "abort"` costs 0.82 MiB over `immediate-abort` (10.61 vs 9.79 MiB
measured): the panic formatting machinery from every dependency. It buys the
panic record's message and location, which `immediate-abort` cannot produce at
all (std aborts before the hook). `panic = "unwind"` costs a further 2.98 MiB
(13.59 MiB) and is rejected; it is the only way rquickjs could catch a panic
in a JS callback (see `docs/upstream.md`).

## Where the bytes are (`.text`, ~8.4 MiB at 12.07 MiB total)

| area | size | note |
| --- | --- | --- |
| `style` (stylo) | 1.3 MiB | upstream structure |
| `blitz_dom` (layout) | 892 KiB | includes taffy grid/flex algorithms |
| `std` | 590 KiB | built in-tree at the profile's flags |
| `browser` (our async plumbing) | 576 KiB | `webdriver::dispatch`, `coordinator_loop`, `cdp::serve_upgraded` |
| `renderer::js` | ~840 KiB | bindings glue (~200 B/member after funnels) plus handwritten DOM logic inlined into dispatch |
| `usvg` / `harfrust` / `skrifa` / `tiny_skia` | 295 / 258 / 227 / 180 KiB | format and raster dependencies |

## Experiments measured flat (do not retry)

- `#[inline(never)]` on generated dispatch groups: flat (outlining relocates code).
- `#[inline(never)]` on conversion helpers alone: flat (arms still inlined their own copies).
- Link-time `--icf=all`: flat (no identical function bodies to fold).
- Dictionary deduplication: 12 structs, 10 unique (`FocusOptions` ×3); negligible.
- Stub probe (all generated arm bodies replaced with `unreachable!()`): 0.87 MiB of `.text` per the earlier 12.19 MiB base, most of it handwritten DOM logic inlined into dispatch monoliths, not glue. That is the ceiling for codegen work, not a target.
