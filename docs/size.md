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
| rquickjs eval | +776KB | JS engine |
| hyper + native-tls | needs re-measure | net (was `ureq +482KB` pre-adoption) |
| url | +197KB | WHATWG URL |
| tungstenite | +113KB | on HTTPS agent |
| tokio rt+time | +66KB | current-thread |
| vello_cpu backend | −1,313,456 | removed 2026-10-06 after Chromium pixel parity (12,030,744 → 10,717,288 stripped); the null-scene probe had estimated ~1.6 MB, the ~0.3 MB gap is the tiny-skia backend plus `image`/`fast_blur` joining the graph |
| encoding/data-url/form/mime handroll promotions | +33,792 | branch vs main, same toolchain 2026-10-09 (10,693,032 → 10,726,824 stripped): `encoding_rs` TextEncoder/TextDecoder + session decoders, `data-url`/`percent-encoding`/`base64` data URLs and atob/btoa, `form_urlencoded` params + submission, `mime` charset sniff, hyper `HeaderMap` |

Watchlist: keep one `JsNode`; no native class per element.
