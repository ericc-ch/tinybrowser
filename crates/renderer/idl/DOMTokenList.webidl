// https://dom.spec.whatwg.org/#interface-domtokenlist
// collections.js installs the value iterator.
[Exposed=Window, Rust=JsTokenList, RustPropertyHooks=Indexed]
interface DOMTokenList {
    [Rust=length] readonly attribute unsigned long length;
    [Rust=item] getter DOMString? item(unsigned long index);
    [Rust=contains] boolean contains([RustFromJs=WebIdlString] DOMString token);
    [CEReactions, Rust=add] undefined add([RustFromJs=WebIdlString] DOMString... tokens);
    [CEReactions, Rust=remove] undefined remove([RustFromJs=WebIdlString] DOMString... tokens);
    [CEReactions, Rust=toggle] boolean toggle([RustFromJs=WebIdlString] DOMString token, optional boolean force);
    [CEReactions, Rust=replace] boolean replace([RustFromJs=WebIdlString] DOMString token, [RustFromJs=WebIdlString] DOMString newToken);
    [Rust=supports] boolean supports([RustFromJs=WebIdlString] DOMString token);
    [CEReactions, Rust=value, RustSet=set_value, RustSetFromJs=WebIdlString] stringifier attribute DOMString value;
    [RustIterable=JavaScript] iterable<DOMString>;
};
