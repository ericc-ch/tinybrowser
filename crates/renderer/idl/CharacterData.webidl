// https://dom.spec.whatwg.org/#interface-characterdata
[Exposed=Window, Rust=JsNode]
partial interface CharacterData {
    [Rust=data, RustSet=set_data] attribute [LegacyNullToEmptyString] DOMString data;
    [Rust=character_data_length] readonly attribute unsigned long length;
    [Rust=substring_data] DOMString substringData(unsigned long offset, unsigned long count);
    [Rust=append_data] undefined appendData(DOMString data);
    [Rust=insert_data] undefined insertData(unsigned long offset, DOMString data);
    [Rust=delete_data] undefined deleteData(unsigned long offset, unsigned long count);
    [Rust=replace_data] undefined replaceData(unsigned long offset, unsigned long count, DOMString data);
};
