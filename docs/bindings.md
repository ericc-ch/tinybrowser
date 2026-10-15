# Web API bindings

Choose language by state ownership, not API name.

Rust owns shared browser state, for example: DOM tree, parsing, layout, event dispatch, I/O, task scheduling, frame/realm lifetimes, origin checks, native lifetimes.

JS requests these through the private host bridge.

JS owns pure algorithms over JS values and private shim state: option
processing, list ops, attribute reflection.

Keep host capabilities and shim state private. Page-replaced methods must not
observe or mutate internals.
