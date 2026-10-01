// https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#xmlserializer
// Rust is a build-time implementation mapping.
[Exposed=Window, Rust=JsXmlSerializer]
interface XMLSerializer {
    [Rust=new] constructor();
    [Rust=serialize_to_string] DOMString serializeToString(Node root);
};
