// https://webidl.spec.whatwg.org/#idl-DOMException
// Rust, RustPrototype, and RustLegacyName are build-time implementation mappings.
[Exposed=Window, Rust=JsDomException, RustPrototype=Error, RustLifetime]
interface DOMException {
    [Rust=new] constructor(optional DOMString message = "", optional DOMString name = "Error");
    [Rust=get_code] readonly attribute unsigned short code;
    [RustField=name] readonly attribute DOMString name;
    [RustField=message] readonly attribute DOMString message;

    [RustLegacyName="IndexSizeError"] const unsigned short INDEX_SIZE_ERR = 1;
    const unsigned short DOMSTRING_SIZE_ERR = 2;
    [RustLegacyName="HierarchyRequestError"] const unsigned short HIERARCHY_REQUEST_ERR = 3;
    [RustLegacyName="WrongDocumentError"] const unsigned short WRONG_DOCUMENT_ERR = 4;
    [RustLegacyName="InvalidCharacterError"] const unsigned short INVALID_CHARACTER_ERR = 5;
    const unsigned short NO_DATA_ALLOWED_ERR = 6;
    [RustLegacyName="NoModificationAllowedError"] const unsigned short NO_MODIFICATION_ALLOWED_ERR = 7;
    [RustLegacyName="NotFoundError"] const unsigned short NOT_FOUND_ERR = 8;
    [RustLegacyName="NotSupportedError"] const unsigned short NOT_SUPPORTED_ERR = 9;
    [RustLegacyName="InUseAttributeError"] const unsigned short INUSE_ATTRIBUTE_ERR = 10;
    [RustLegacyName="InvalidStateError"] const unsigned short INVALID_STATE_ERR = 11;
    [RustLegacyName="SyntaxError"] const unsigned short SYNTAX_ERR = 12;
    [RustLegacyName="InvalidModificationError"] const unsigned short INVALID_MODIFICATION_ERR = 13;
    [RustLegacyName="NamespaceError"] const unsigned short NAMESPACE_ERR = 14;
    [RustLegacyName="InvalidAccessError"] const unsigned short INVALID_ACCESS_ERR = 15;
    const unsigned short VALIDATION_ERR = 16;
    [RustLegacyName="TypeMismatchError"] const unsigned short TYPE_MISMATCH_ERR = 17;
    [RustLegacyName="SecurityError"] const unsigned short SECURITY_ERR = 18;
    [RustLegacyName="NetworkError"] const unsigned short NETWORK_ERR = 19;
    [RustLegacyName="AbortError"] const unsigned short ABORT_ERR = 20;
    [RustLegacyName="URLMismatchError"] const unsigned short URL_MISMATCH_ERR = 21;
    [RustLegacyName="QuotaExceededError"] const unsigned short QUOTA_EXCEEDED_ERR = 22;
    [RustLegacyName="TimeoutError"] const unsigned short TIMEOUT_ERR = 23;
    [RustLegacyName="InvalidNodeTypeError"] const unsigned short INVALID_NODE_TYPE_ERR = 24;
    [RustLegacyName="DataCloneError"] const unsigned short DATA_CLONE_ERR = 25;
};
