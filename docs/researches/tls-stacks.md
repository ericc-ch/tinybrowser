# TLS stack comparison

Three client stacks were compared for size and for observable blocking:
`ureq` 3.4 with `native-tls` (v1's stack, system OpenSSL), a hyper client over
`hyper-rustls` with `ring` (today's stack), and `wreq` 6.0.0-rc with BoringSSL
and the `Chrome149` emulation profile (JA3/JA4 and HTTP/2 signature parity).

Probes live in `tools/tlsprobe` (a standalone crate, not a workspace member).
Each binary reports the same endpoints. `PROBE_CHROME_UA=1` adds a Chrome user
agent and header set to the first two probes, which separates a TLS
fingerprint effect from a header effect. The shipping browser was measured
through CDP navigation to the same endpoints.

## Size

Built with the probe's release profile (`opt-level = "z"`, fat LTO,
`codegen-units = 1`, `strip`, `panic = "abort"`); the repository's
`.cargo/config.toml` ICF flag also applies because cargo walks parent
directories for configuration.

| Probe | Stripped bytes | Delta |
| --- | ---: | ---: |
| `ureq` + `native-tls` (v1) | 754,496 | — |
| hyper + `hyper-rustls`/`ring` (current) | 1,829,144 | **+1,074,648** |
| `wreq` + BoringSSL + `Chrome149` | 3,791,880 | +1,962,736 |

These are whole clients, so the deltas include the HTTP stacks (ureq vs hyper
vs wreq). They line up with the earlier in-tree numbers: the preflight
isolated TLS delta for hyper-rustls/ring was 1,011,960 bytes, and the shipping
binary is 6,582,320 bytes today. A Chrome-emulating stack would add roughly
two more megabytes on top of the current client, not a few hundred kilobytes.

## Fingerprints

`https://tls.browserleaks.com/json`, one run per probe:

| Stack | JA3 | JA4 | Akamai (HTTP/2) |
| --- | --- | --- | --- |
| `native-tls` | `0b85eb0d4981e69064e40753e4f0ac5f` | `t13d301100_1d37bd780c83_8e6e362c5eac` | (HTTP/1.1, none) |
| `hyper-rustls`/`ring` | `3dfe4d12def5ec19165f6bb6d0027c28` | `t13d1011h2_61a7ad8aa9b6_3fcd1a44f3e3` | `9b5dcd077a77c5e324b91d8cc306fd5a` |
| `wreq` Chrome149 | `0a3867b0dcdd6303db37850ed29fd5cb` | `t13d1516h2_8daaf6152771_d8a2da3f94cd` | `52d84b11737d980aef856699f885ca86` |

The shipping browser reports the same rustls family as the current probe
(`ja4 t13d1011h2_61a7ad8aa9b6_3fcd1a44f3e3`, `akamai 9b5dcd07...`), so what we
measured with the probe is what the product sends. JA3 hashes move between
runs for rustls (session resumption changes the extension list); JA4 is the
stable identifier.

## Blocking

Status codes, same run window and source address:

| Endpoint | native-tls | rustls/ring | Chrome149 | rustls + Chrome UA | native + Chrome UA |
| --- | --- | --- | --- | --- | --- |
| `tls.peet.ws/api/all` | 200 | 200 | 200 | 200 | 200 |
| control `httpbin.org/get` | 200 | 200 | 200 | 200 | 200 |
| `nopecha.com/demo/cloudflare` | 403 | 403 | 403 | 403 | 403 |
| `www.g2.com` | 403 | 403 | 403 | 403 | 403 |
| `www.reddit.com` | 200 | **403 Blocked** | 200 | **200** | **200** |
| shipping browser, Reddit | — | **403 Blocked** (empty `User-Agent`) | — | — | — |

The one reproducible block was **not** about TLS. Reddit rejects requests with
an empty `User-Agent`; adding a Chrome user agent and accept headers made both
the native and rustls stacks pass. Cloudflare's Turnstile and DataDome
challenges at `nopecha.com` and `g2.com` reject every bare client regardless of
TLS stack because they need JavaScript execution. `nowsecure.nl` no longer
serves a Cloudflare interstitial at all, so it was dropped from the table.

## Conclusion

BoringSSL with a Chrome emulation profile buys a Chrome-family JA3/JA4 and
HTTP/2 signature at roughly twice the size of the current client. In these
tests it did not unblock anything the current stack could not reach once the
request carried a browser-like `User-Agent`; the failures we could reproduce
were header policy and JavaScript challenges, neither of which is a TLS
property. The current rustls/ring choice stays.

If a future requirement proves JA3/JA4 enforcement against us, or needs
Chrome-identical TLS behavior, `tools/tlsprobe` already prices the swap:
`wreq` plus BoringSSL measured +1,962,736 bytes over the current probe, and
would be a deliberate project (C++/CMake build, `aws-lc-rs` or `boring` as the
provider, and every protocol gate re-run).
