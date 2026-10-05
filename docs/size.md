# Size

Policy: minimize stripped x86_64 size; no hard cap. Latest size in
`progress.md`. Reproduce with `tools/release`.

## Method

Throwaway probe that really exercises the dep, delta vs empty-`main`
of the same profile (`opt-level="z"`, fat LTO, `strip`, `panic="abort"`,
lld `--icf=all`; release-bin RELR + `.eh_frame` removal in `tools/release`).

## Marginals

| Dep | Delta | Note |
| --- | ---: | --- |
| html5ever + tree | +840KB | full tree build |
| selectors + cssparser | +75KB | cheap |
| rquickjs eval | +776KB | JS engine |
| ureq + native-tls | +482KB | net |
| url | +197KB | WHATWG URL |
| tungstenite | +113KB | on HTTPS agent |
| tokio rt+time | +66KB | current-thread |
| tiny-skia / png | +214KB / +86KB | paint + encode |
| Taffy layout | +304KB | probe |
| Parley + skrifa | +883KB | shaping; floor |
| Blitz stack | +6.7MB | pre-adoption full-stack probe (window/GPU wiring); adopted shape differs, see below |
| vello_cpu backend | −1,313,456 | removed 2026-10-06 after Chromium pixel parity (12,030,744 → 10,717,288 stripped); the null-scene probe had estimated ~1.6 MB, the ~0.3 MB gap is the tiny-skia backend plus `image`/`fast_blur` joining the graph |

Watchlist: keep one `JsNode`; no native class per element.
