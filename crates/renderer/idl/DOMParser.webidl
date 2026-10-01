// https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-parsing-and-serialization
// Rust is a build-time implementation mapping.
[Exposed=Window, Rust=JsDomParser]
interface DOMParser {
    [Rust=new] constructor();
    [NewObject, Rust=parse_from_string] Document parseFromString((TrustedHTML or DOMString) string, DOMParserSupportedType type);
};

enum DOMParserSupportedType {
    "text/html",
    "text/xml",
    "application/xml",
    "application/xhtml+xml",
    "image/svg+xml"
};
