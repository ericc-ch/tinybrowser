# Web API bindings

Choose language by state ownership, not API name.

Rust owns shared browser state, for example: DOM tree, parsing, layout, event dispatch, I/O, task scheduling, frame/realm lifetimes, origin checks, native lifetimes.

JS requests these through the private host bridge.

JS owns pure algorithms over JS values and private shim state: option
processing, list ops, attribute reflection.

Keep host capabilities and shim state private. Page-replaced methods must not
observe or mutate internals.


## `[SecureContext]`

The database resolves the fact once: a member is secure when `[SecureContext]`
is on the member, on its interface, or on a partial interface or mixin it comes
from, and both emitters read that one `Exposure`. Neither emitter decides from
the annotation's position.

The native path installs the member tables through `host::install_members`,
which reads the realm's bit once and skips gated members; a `[SecureContext]`
interface answers `None` from `prototype` and `constructor`, so its interface
object never exists. The JS path carries `exposure.secureContext` into
`__tbInstallInterface`, which builds the interface prototype from the member
list and defines only the exposed members, so an unexposed member is absent
rather than present and throwing (<https://webidl.spec.whatwg.org/#SecureContext>).

A JS interface must name every IDL member it does not implement in the
installation's second argument, and the build fails otherwise. `Crypto` lists
`['subtle']`: the algorithms are absent until the engine can implement them
faithfully. A gated interface object on the JS path, mixin `includes`
resolution, and structured type lowering are tracked in #212.

`isSecureContext` is a Rust accessor in `js/bridge.rs`; the engine has no
`Window` platform object to install `WindowOrWorkerGlobalScope` onto.
