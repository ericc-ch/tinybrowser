// https://dom.spec.whatwg.org/#mutation-observers
// Rust and RustThis are build-time implementation mappings.
callback MutationCallback = undefined (sequence<MutationRecord> records, MutationObserver observer);

dictionary MutationObserverInit {
    [Rust=child_list] boolean childList = false;
    [Rust=attributes] boolean attributes;
    [Rust=character_data] boolean characterData;
    [Rust=subtree] boolean subtree = false;
    [Rust=attribute_old_value] boolean attributeOldValue;
    [Rust=character_data_old_value] boolean characterDataOldValue;
    [Rust=attribute_filter] sequence<DOMString> attributeFilter;
};

[Exposed=Window, Rust=JsMutationObserver]
interface MutationObserver {
    [Rust=create] constructor(MutationCallback callback);
    [Rust=observe, RustThis] undefined observe(Node target, optional MutationObserverInit options = {});
    [Rust=disconnect] undefined disconnect();
    [Rust=take_records] sequence<MutationRecord> takeRecords();
};
