// https://dom.spec.whatwg.org/#interface-eventtarget
// Rust, RustThis, and RustValue are build-time implementation mappings.
[Exposed=Window, Rust=JsEventTarget]
interface EventTarget {
    [Rust=new] constructor();

    [Rust=add_event_listener, RustThis] undefined addEventListener(DOMString type, [RustValue] EventListener? callback, [RustValue] optional (AddEventListenerOptions or boolean) options = {});
    [Rust=remove_event_listener, RustThis] undefined removeEventListener(DOMString type, [RustValue] EventListener? callback, [RustValue] optional (EventListenerOptions or boolean) options = {});
    [Rust=dispatch_event, RustThis] boolean dispatchEvent([RustValue] Event event);
};
