# Size budget

Research: why the binary stays small, how to measure a dependency, and what
was kept or rejected. Shipping binary size lives in
[`docs/progress.md`](../progress.md).
Target: under 10MB stripped on x86_64.

## Method

Reproduce each dependency with a throwaway probe that really exercises it
(tokenize, dial, JS eval, parse+query). Marginal is the delta versus an
empty-`main` build of the same profile. rustc 1.98 on Linux unless a row
says otherwise.

The tuned profile is `opt-level = "z"`, `lto = "fat"`,
`codegen-units = 1`, stripped, `panic = "abort"`, plus lld `--icf=all`.
Those flags live in the root `Cargo.toml` and `.cargo/config.toml` so
`cargo build --release` matches this research.

## Binding and JS size

One `JsNode` plus JS brands. Not a native class per `HTML*Element`. Host
objects and spec algorithms that touch the node tree or Event state are a
Rust binding. Web IDL sugar stays JS. That split is the size and perf lever
for DOM glue. Avoid hundreds of rquickjs classes.

Intl: option reads are JS (`INSTALL_INTL_JS`, about 31KB). Format is a Rust
binding. The locale-filtered ICU4X blob is 125,426 bytes (`en-US`, `es-ES`,
`de-DE`, `ja-JP`, `fr-FR`, `zh-CN`, `ko-KR`). Marker-based generation keeps
unused datasets out.

## Chosen vs rejected (tuned probes)

| Choice | Tuned | Why |
| --- | ---: | --- |
| html5ever 0.39 + tree builder | +840 KB | Full HTML5 tree construction. html5gum is tokenizer-only (+272 KB) and would need weeks of spec work. |
| selectors + cssparser on html5ever | +75 KB | Cheap. Whole dom stack (arena + selectors + html5ever) later measured +932 KB. |
| rquickjs 0.12 (eval + limits) | +776 KB | JS engine. |
| ureq 3 + native-tls (dyn `libssl.so.3`) | +482 KB | Net v1. Shipping TLS is still native-tls (ALPN h2) behind the connector seam. |
| url 2.x | +197 KB | WHATWG URL. |
| tungstenite handshake | +184 KB standalone, +113 KB on the HTTPS agent | WebSocket. |
| tokio `rt`+`time` current-thread | +66 KB | Enough for timers. `full` is +208 KB. Smol is not smaller (+142 KB). |
| wreq realistic | +4020 KB | Rejected. Puts the stack near 6 MB with CDP still unaccounted. |
| ureq bridged to btls | +1797 KB | Rejected. h1-only, buys neither ship-soonest nor Chrome-shaped bytes. |
| btls standalone | +1302 KB | Deferred stealth target (hand-rolled h1/h2), not shipping. |
| hyper-rustls + ring (isolated TLS) | +1,011,960 | Preflight. Shipping later dropped rustls/ring for native-tls. |

Bot blockers fingerprint TLS ClientHello, HTTP/2 settings, and header
order. Canonical-Chrome wire behavior stays the target. Stealth is deferred
to btls, not dropped. Pin that crate family like html5ever. Presets go
stale with every Chrome release.

A 2026-08-25 live matrix (16 targets) was a checkpoint method, not a
standing truth: ureq+native-tls 9/16, ureq+btls 10/16, wreq Chrome148 11/16
plus 3 ambiguous redirects. IP reputation (g2.com, canadagoose) blocked
every client.

## Size levers

Kept, measured against the P14 shipping binary (7,414,144 bytes):

| Lever | Bytes | Delta |
| --- | ---: | ---: |
| `panic = "abort"` | 6,652,624 | −761,520 |
| + lld `--icf=all` | 6,582,896 | −69,728 |

A Rust panic ends the daemon. A renderer child aborts and the browser
reaps it. rquickjs never uses unwinding for JS exceptions.

The knob set below includes `--no-eh-frame-hdr` and C `-fno-unwind-tables`,
so unwind profiles (dev, tests) also abort on panic instead of unwinding:
`catch_unwind`-based recovery and `#[should_panic]` no longer work in tests.
The shipping binary is unaffected: it is built with `panic = "abort"`, and
nothing in the graph unwinds through foreign code (QuickJS uses setjmp and
longjmp).

Rejected or deferred:

| Lever | Cost | Why not |
| --- | ---: | --- |
| `relocation-model=static` | −490,568 | Loses ASLR. |
| Drop `webpki-roots` fallback | −75,200 | Loses the root-store fallback. |
| TLS 1.3 only | −46,816 | Drops TLS 1.2 servers. |
| `opt-level = "s"` + abort + ICF | +379,248 | `"z"` is smaller. |
| CDP on hyper-direct, not axum | −179,568 shipped (see the knobs section) | Shipped 2026-09-18. |
| Hand-rolled CLI, not clap | −184,912 shipped (see the knobs section) | Shipped 2026-09-18. |
| Feature-gate Intl | 125,426 blob plus code | Removes the Intl surface. |
| Drop HTTP/2 | 63.4 KiB `.text` plus hyper paths | Loses HTTP/2. |
| `-Z build-std` + `panic_immediate_abort` | about 625 KiB of Rust async unwind tables (see the knobs section) | Nightly. Workspace pins stable 1.98. |

## Perf: process cost

Blank targets share a renderer. Per-tab processes used 925 file descriptors
(near a 1024 soft limit) and 210 MB PSS. Virtual blanks land at 3
processes, 10 threads, 33 descriptors, 11.6 MB PSS.

native-tls touches about 4.5 MB of `libcrypto` before any dial while
building TLS contexts. Handshake count does not grow that footprint. The
fixed shared-library cost is accepted.

## Watchlist

- DOM/JS glue: keep one `JsNode`. Do not add a native class per element.
- A11y walker: budget about 100–200 KB. Measure when it lands.
- Stealth on btls: pin the crate family. Refresh persona tables with
  Chrome, not only crate versions.
- Axum vs hyper-direct, clap vs a hand-rolled CLI, Intl gating, HTTP/2:
  costs in the rejected table. Product decisions, not free size.
- Probe marginals belong here. Shipping binary size belongs in
  `docs/progress.md`.

## Screenshot render stack (2026-09-18)

Screenshots are always in: the shipping binary carries the render pipeline.
The Blitz measurement above stands (lean layout alone was ~10.8 MB before
paint), so the shipped shape is an in-tree CSS subset instead of an
integrated engine: `crates/render` parses and cascades CSS, lays out block,
inline, and flex formatting, paints with `tiny-skia`, rasterizes text with
`fontdue`, and encodes PNG with `png`.

Probe deltas on an empty binary with the tuned profile (rustc 1.98.1,
x86_64-unknown-linux-gnu, lld `--icf=all`):

| Crate | Stripped delta |
| --- | ---: |
| tiny-skia 0.12 | +213,360 |
| fontdue 0.9 | +120,504 |
| png 0.17 | +86,008 |
| all three together | +393,880 |

Shipping delta: 5,988,848 -> 6,606,240 bytes (+617,392), which includes the
embedded subset faces (Liberation Sans Regular 29,680 + Bold 29,896 bytes,
OFL-1.1, `crates/render/assets/OFL.txt`) and the crate's own style, layout,
paint, and PNG code.

Selector matching reuses `dom`'s pinned `selectors`/`cssparser` stack through
a compile-once API (`Dom::compile_selectors`), so the cascade adds no second
selector version. Deliberately not shipped: fontconfig/system fonts (one
embedded subset instead), and any Blitz, Stylo, Taffy, Parley, or
vello/anyrender dependency.

## Taffy box layout (2026-09-18)

The hand-rolled block/flex engine (`layout.rs` block flow, `flex.rs`) is
replaced by Taffy 0.14 (`crates/render/src/boxes.rs`): block flow with margin
collapsing, flex, floats, and absolute positioning. Inline formatting stays
in-tree, measured through Taffy's measure hooks. Isolated probe on an empty
tuned binary: +303,848 bytes. Shipping delta: 6,606,240 -> 7,034,176 bytes
(+427,936), which includes the `boxes.rs` conversion and measure glue minus
the deleted `flex.rs`. Taffy 0.14 dropped the CSS `order` property, so flex
children are stable-sorted by `order` at tree-build time. Grid is the next
slice (template properties first, then placement mapping).

## Grid track parsing (2026-09-18)

`grid-template-columns/rows` (lengths, `fr`, `auto`, `minmax()`,
`repeat()`), `grid-column/row` line placement, and `justify-items` ride the
existing Taffy dependency: no new crates. Shipping delta: 7,034,176 ->
7,051,360 bytes (+17,184). Dropping `Copy` from `Style` (grid templates own
a `Vec`) cost nothing measurable.

## Parley text shaping (2026-09-18)

The `fontdue` bitmap path and the hand-rolled line breaker are replaced by
Parley 0.11 (shaping, bidi, UAX#14 breaking, alignment) with `skrifa`
outlines filled by `tiny-skia`. No system fonts: the embedded subset is
registered from memory, and `parley`/`fontique` build with default features
off plus `libm`. Isolated probe on an empty tuned binary: +882,768 bytes.
Shipping delta: 7,051,360 -> 8,118,512 bytes (+1,067,152, rustc 1.98.0), the difference
being the shaping driver, the skrifa-direct outline path, and feature
unification across the workspace. `fontdue` stays for intrinsic width
measurement only.

## Linker and C-flag knobs (2026-09-18)

Measured on the screenshot stack. The pre-knob baseline
(`8,118,576` bytes here; shipping recorded `8,118,512` at Parley head;
64 bytes are data-layout noise) was built with rustc 1.98.0; every other
number in this section is rustc 1.98.1, so the knob row carries a small
toolchain term (the same tree is `7,607,136` bytes under 1.98.1, −511,440 of
which is not the flags). Each probe builds the exact tuned profile with one
change; the kept set was re-verified through the committed
`.cargo/config.toml` (a rebuild with config-delivered flags finished fresh in
about a second, byte-identical output).

Kept flags (section deltas measured against the pre-knob baseline):

| Lever | Bytes | Section |
| --- | ---: | --- |
| `-Wl,--no-eh-frame-hdr` | −98,348 | deletes `.eh_frame_hdr` outright |
| `CFLAGS="-fno-unwind-tables -fno-asynchronous-unwind-tables"` | −53,032 | deletes QuickJS-ng's `.eh_frame` (the only C compiled in the graph) |
| lld `-O2` | −1,856 | `.rodata` string merging |

Within layout noise: `-Wl,--build-id=none` and `-Wl,--gc-sections`. The
baseline already carried no GNU build-ID note, and LTO already collected
unused sections, so those two rows are intent pins, not measured wins.

Then, measured within rustc 1.98.1:

| Lever | Bytes | Section |
| --- | ---: | --- |
| Hand-rolled CLI, not clap (`src/cli.rs`) | −184,912 | four global flags and three subcommands; parity pinned by `tests/modes.rs` |
| Hyper-direct servers, not axum (cdp, webdriver) | −173,360 | hyper-util server-auto + tokio-tungstenite upgrade; direct base64 deps unified on 0.23 |

The hyper-direct port briefly regressed behavior; the parity fixes landed on
top (shutdown race check, 404/405 routing, HEAD kept to hyper, RFC 6455
version validation, a size-limited WebDriver body, accept retry with
backoff) and cost 2,592 bytes. Real breakage the reviewers measured in the
first cut: a `watch` receiver created after the stop signal never fired, so
`serve` could hang; the WebDriver cap was applied after buffering the whole
body; `POST` to unknown paths answered 405 instead of 404; HEAD lost
`Content-Length`; and a missing or wrong `Sec-WebSocket-Version` upgraded
anyway. Behavior parity with the old axum stack is re-verified by raw probes
plus the Blink CDP and Playwright gates.

Nothing unwinds at runtime under `panic = "abort"` (QuickJS uses
setjmp/longjmp), so the unwind index and the C tables are dead weight.
Backtraces through our frames lose symbolization, and the flags also disable
panic recovery in dev and test binaries (the flag set lives in
`.cargo/config.toml` for every profile). The full workspace test suite (32
suites) passes with the complete flag set, and a live daemon (renderer child
spawn, CDP `/json/version` + `/json/list`) is clean.

Dead ends: `-C force-unwind-tables=no` (−128; sync tables are already gone
via `panic = "abort"`, `.gcc_except_table` is 6.8 KiB — the remaining
`.eh_frame` is async unwind data, which the flag does not govern),
`-z norelro` (−112; writable GOT, rejected), `strip = "symbols"`
(identical to `true` on ELF), `-C llvm-args=-unwind-tables=0` (LLVM rejects
the flag; stable has no handle on async tables). The remaining Rust async
unwind tables (about 625 KiB here, including precompiled `std`) still need
nightly `-Z build-std`.

## Stylo cascade (2026-09-18)

The isolated probe said stylo costs +1.33 MB; the realized delta on top of
the Parley tree was +2.00 MB (8,118,512 -> 10,123,008 at the time), because
the probe never instantiated the full cascade and property database. The
breakdown at 10,123,008: `.text` +1.51 MB, `.eh_frame` +263 KB, `.rela.dyn`
+67 KB, `.rodata` +88 KB. `-C force-unwind-tables=no` is a no-op here with
fat LTO (std's bitcode carries the uwtable module flag), and no stable knob
reaches the async unwind tables, so the Stylo frame data is part of the
price.

Two real savings landed while fitting it:

| Change | Bytes | Why |
| --- | ---: | --- |
| One Taffy measure closure (`Builder::run_layout`) | −155,000 | the closure type is part of Taffy's generic instantiation; two entry points meant two copies of grid/flex/block compute code |
| Drop `fontdue`; intrinsic advances from `skrifa` | −73,000 | one font stack instead of two; `Fonts::measure` reads `hmtx` directly |
| `skrifa` without `autohint_shaping` | −224 | outlines are drawn unhinted |

Rebased onto `chase/size-flags` (7,603,520 bytes), the Stylo slice lands at
**9,416,800 bytes**: 1,813,280 over the size-flags base, 583,200 under the
cap. The pre-rebase clap feature trimming is gone with clap itself.

## Full Blitz adoption (2026-09-18)

Priced to settle the paint question: keep building on tiny-skia, or take
Blitz's painters. All probes use the shipping profile (rustc 1.98.1, lld
`--icf=all`); each delta is stripped size minus the same-folder empty binary
(293,312 bytes).

| Probe | Bytes | Delta |
| --- | ---: | ---: |
| anyrender scene recorder only (null backend) | 296,720 | +3,408 |
| tiny-skia (rect, circle, gradient, transform) | 508,088 | +214,776 |
| anyrender + anyrender_vello_cpu (rounded rect, gradient, clip, layer alpha) | 1,774,352 | +1,481,040 |
| blitz-dom + blitz-paint + anyrender_vello_cpu, 300x200 document rendered | 7,042,560 | +6,749,248 |

`blitz-paint::paint_scene` accepts only a `blitz_dom::BaseDocument`, and
anyrender has no tiny-skia backend (vello, vello_cpu, vello_hybrid, skia,
svg, null), so "Blitz's paint into tiny-skia" is not a configuration that
exists; adopting Blitz's paint means adopting its DOM/layout stack and the
vello_cpu rasterizer.

Net swap, pricing our side with a stub of the `render` API (stub tinybrowser
= 5,539,584 bytes, everything else identical): the shipped pipeline costs
3,877,216 bytes (9,416,800 - 5,539,584). Replacing it with the Blitz stack is
5,539,584 + 6,749,248 = **~12.29 MB**, i.e. +2.87 MB over today and about
2.3 MB over the cap.

Caveats: the Blitz probe runs with `default-features = false` (no system
fonts, accessibility, SVG, WOFF, custom widgets, scrollbars, floats, complex
scripts), so the figure is a floor; system-fonts additionally wants
fontconfig, which the dev shell does not carry. The probe also keeps our
`dom`/JS bindings in place; adopting Blitz's DOM in its place is a separate
refactor whose size is not counted. The extra budget does buy feature
surface we lack (tables, replaced content, gradients/radii/shadows,
stacking contexts), so this is a price for capability, not pure overhead.
