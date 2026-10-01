// https://dom.spec.whatwg.org/#interface-event
// Rust and RustSet are build-time implementation mappings.
dictionary EventInit {
    [Rust=bubbles] boolean bubbles = false;
    [Rust=cancelable] boolean cancelable = false;
    [Rust=composed] boolean composed = false;
};

[Exposed=Window, Rust=JsEvent]
interface Event {
    [Rust=new] constructor(DOMString type, optional EventInit eventInitDict = {});

    [Rust=get_type] readonly attribute DOMString type;
    [Rust=get_target] readonly attribute EventTarget? target;
    [Rust=get_src_element] readonly attribute EventTarget? srcElement; // legacy
    [Rust=get_current_target] readonly attribute EventTarget? currentTarget;
    // UI Events defines relatedTarget on FocusEvent; this runtime does not
    // model a separate focus-event class, so the focus events reuse Event
    // (<https://w3c.github.io/uievents/#dom-focusevent-relatedtarget>).
    [Rust=get_related_target] readonly attribute EventTarget? relatedTarget;
    [Rust=composed_path] sequence<EventTarget> composedPath();

    const unsigned short NONE = 0;
    const unsigned short CAPTURING_PHASE = 1;
    const unsigned short AT_TARGET = 2;
    const unsigned short BUBBLING_PHASE = 3;
    [Rust=get_event_phase] readonly attribute unsigned short eventPhase;

    [Rust=stop_propagation] undefined stopPropagation();
    [Rust=get_cancel_bubble, RustSet=set_cancel_bubble] attribute boolean cancelBubble;
    [Rust=stop_immediate_propagation] undefined stopImmediatePropagation();

    [Rust=get_bubbles] readonly attribute boolean bubbles;
    [Rust=get_cancelable] readonly attribute boolean cancelable;
    [Rust=get_return_value, RustSet=set_return_value] attribute boolean returnValue;
    [Rust=prevent_default] undefined preventDefault();
    [Rust=get_default_prevented] readonly attribute boolean defaultPrevented;
    [Rust=get_composed] readonly attribute boolean composed;

    [LegacyUnforgeable, Rust=get_is_trusted] readonly attribute boolean isTrusted;
    [Rust=get_time_stamp] readonly attribute DOMHighResTimeStamp timeStamp;

    [Rust=init_event] undefined initEvent(DOMString type, optional boolean bubbles = false, optional boolean cancelable = false);
};
