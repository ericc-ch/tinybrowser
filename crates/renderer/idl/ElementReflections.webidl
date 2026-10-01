// Element-level content-attribute reflections that the runtime has always
// exposed on every element prototype (`name`, `href`, `src`, `content`). The
// HTML spec places these on individual element interfaces; they are grouped
// here so the reflection stays shared, matching the existing surface, rather
// than duplicated across the element interfaces.
[Exposed=Window, Rust=JsNode, RustOwnedCtx, RustInstall="Element"]
partial interface ElementReflections {
    [CEReactions, Rust=name, RustSet=set_name, RustSetFromJs=WebIdlString] attribute DOMString name;
    [CEReactions, Rust=href, RustSet=set_href, RustSetFromJs=WebIdlString] attribute DOMString href;
    [CEReactions, Rust=src, RustSet=set_src, RustSetFromJs=WebIdlString] attribute DOMString src;
    [RustValue, Rust=content, RustSet=set_content, RustSetFromJs=WebIdlString] attribute DOMString content;
};
