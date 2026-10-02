# Parser source

This directory vendors `sagudev/weedle4` at commit
`7dd96eac546da66a99b2bf1abe8a13a977f60071`.
The crate is a build dependency. It does not ship in the browser binary.

The downstream grammar adds the `async_sequence` terminal and type from
[Web IDL](https://webidl.spec.whatwg.org/#idl-async-iterable-type).
The AST keeps async sequences distinct from synchronous sequences.
The binding generator rejects implemented conversions that it cannot generate.

The renderer build parses every pinned non-tentative WPT extract.
That build checks the parser against the imported inputs.
Generator fixtures live in `crates/webidl-bindgen/tests/`.
