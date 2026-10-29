# Web API bindings

Choose language by state ownership, not API name.

Rust owns shared browser state, for example: DOM tree, parsing, layout, event dispatch, I/O, task scheduling, frame/realm lifetimes, origin checks, native lifetimes.

JS requests these through the private host bridge.

JS owns pure algorithms over JS values and private shim state: option
processing, list ops, attribute reflection.

Keep host capabilities and shim state private. Page-replaced methods must not
observe or mutate internals.

## `[SecureContext]`

The generator owns the gate: `[SecureContext]` on a JS-path attribute or
operation is carried into the install descriptor, and
`__tbInstallInterface` deletes the member from the prototype when the realm's
bit is false. The class body supplies the algorithm; the generator supplies
its absence, which is what Web IDL requires
(<https://webidl.spec.whatwg.org/#SecureContext>). `crypto.randomUUID` is the
first member on this path.

`isSecureContext` itself is the Rust accessor in `js/bridge.rs`: the engine has
no `Window` platform object, so there is no `WindowOrWorkerGlobalScope` mixin
install to hang a generated accessor on.

`navigator.userAgentData` still installs through the shim's captured bit.
Moving it needs two generator features the JS path does not have: resolving
`Navigator includes NavigatorUA` (members currently come from the interface's
own declaration only) and lowering `FrozenArray<...>`, dictionary arrays,
`Promise<...>`, and interface-valued attributes. The shim captures the bit at
install, so a page cannot turn the member back on.
