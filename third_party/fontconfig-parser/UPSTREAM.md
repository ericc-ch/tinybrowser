# fontconfig-parser

Vendored from crates.io `fontconfig-parser` 0.5.8
(<https://github.com/Riey/fontconfig-parser>). The Rust sources are that
release. The only manifest change is `roxmltree = "0.21.1"` (upstream
requires `0.20.0`).

`usvg` 0.48.1 and `fontique` 0.11.1 already require `roxmltree` 0.21.1.
No newer `fontconfig-parser` release relaxes its pin, so Cargo was linking
both parsers. Patching this crate onto 0.21 is one version requirement.
`png` 0.18.1 still requires `miniz_oxide` 0.8 and `flate2` 1.1.10 requires
0.9, so that pair is not the same kind of bump.

The calls in this crate (`Document::parse`, `parse_with_options`,
`ParsingOptions` with `allow_dtd`, `Node`, `Error`, `Attribute::name`)
still compile against 0.21. Upstream's own tests, including the fontconfig
JSON fixtures, pass on 0.21.1 with these sources unchanged.
