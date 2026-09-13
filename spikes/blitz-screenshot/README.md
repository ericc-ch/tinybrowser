# Blitz screenshot spike

Throwaway probe behind the 2026-09-13 Blitz measurement on
`spike/blitz-screenshot`. Not part of the shipping build; the workspace
`Cargo.toml` excludes `spikes/`.

It answers the note `projects/tinybrowser/screenshot.md` (Obsidian): can
one-shot HTML -> style -> layout -> CPU raster -> PNG ride along in the one
binary inside the 10 MB budget?

## What it does

`src/main.rs` renders an HTML file to a PNG in one pass:

1. `blitz-html` (html5ever) parses the document.
2. `blitz-dom` runs Stylo 0.20 for the cascade and Taffy 0.14 for layout.
3. `blitz-paint` pushes a display list into `anyrender`.
4. `anyrender_vello_cpu` (vello_cpu 0.1) rasterizes one RGBA buffer.
5. `png` encodes one file.

Text uses a single supplied TTF via `build_single_font_ctx`, so system font
discovery (fontconfig) stays off. Also here:

- `src/bin/empty.rs` - size baseline.
- `src/bin/layout_only.rs` - parse + style + layout rung, no paint.
- `test-page.html` - flex, grid, gradient, radius, shadow, text, table.

## Run it

The flake dev shell carries everything the stack needs (Python for Stylo's
build script, cargo-bloat, lld, upx, xz).

```sh
nix develop --command cargo build --release \
    --manifest-path spikes/blitz-screenshot/Cargo.toml

./spikes/blitz-screenshot/target/release/blitz-screenshot \
    spikes/blitz-screenshot/test-page.html /tmp/out.png \
    /nix/store/0lhf2h2fnjqvpzmk92s5axr2ybvs981s-dejavu-fonts-2.37/share/fonts/truetype/DejaVuSans.ttf
```

The same pipeline is wired into the CLI behind the `screenshot` feature:

```sh
nix develop --command env PYTHON3="$PYTHON3" \
    cargo build --release --bin tinybrowser --features screenshot

./target/release/tinybrowser screenshot test-page.html /tmp/out.png \
    --font /path/to/font.ttf
```

## Result

Measured with rustc 1.98.0, tuned profile, stripped (the committed spike
profile now also carries `panic = "abort"`). Full numbers and the budget
consequence live in
[`docs/researches/size-budget.md`](../../docs/researches/size-budget.md#spike-blitz-one-shot-screenshot-2026-09-13).

| Rung | panic=unwind | panic=abort |
| --- | ---: | ---: |
| empty | 290,944 | 289,256 |
| Blitz layout | 6,236,256 | 5,602,832 |
| + vello paint + PNG | 8,573,248 | 7,885,192 |
| shipping CLI baseline | 5,810,304 | 5,282,784 |
| CLI + feature, layout only | 10,788,304 | not measured |
| CLI + feature, full | 13,113,008 | 11,922,640 |

The layout rung alone busts the 10,000,000 byte ceiling; a cheaper paint
backend cannot close the gap by itself.

## Trim experiments

All measured on this branch, same profile:

| Lever | Effect | Cost |
| --- | ---: | --- |
| `panic = "abort"` | −1.19 MB on the full CLI, −528 KB feature-off | rquickjs `catch_unwind` stops turning Rust panics into JS errors |
| `-C relocation-model=static` | −574,240 B on the probe (net) | non-PIE: no ASLR |
| ICF (`lld --icf=all`) | −107,392 B on the probe | lld in the toolchain; PIE kept |
| `-C force-unwind-tables=no` | 0 B after stripping | none |
| UPX `--best --lzma` | 11,922,640 → 3,716,652 B (31.2%) | ~186 ms per process start |

From `cargo bloat` (probe, `.text` = 5.0 MiB): Stylo 1.4 MiB, vello_cpu
1.0 MiB, blitz_dom 711 KiB, std 426 KiB, Parley stack (skrifa + harfrust +
read_fonts + fontique + parley) 690 KiB. Isolated probes: `fontdue` costs
+116 KB against that Parley stack (~570 KB saving, no complex shaping);
`image` default codecs cost −1.07 MB but blitz-dom already avoids them.

Under-10 MB scenario with Blitz: abort → non-PIE → tiny-skia paint backend →
fontdue text → ICF lands at ~9.7 MB. Every lever is required, including the
panic-policy and ASLR tradeoffs.

## Not tried here

- Blitz `system-fonts` (fontconfig) build; the single-font path was chosen on
  purpose for the one-binary constraint.
- A tiny-skia `anyrender::PaintScene` backend, the note's locked paint choice.
- Network resources (images, external CSS): no `blitz-net` provider is wired,
  so only inline styles and data URIs load.
- Real-site pages; `test-page.html` covers the CSS features a first cut needs.
