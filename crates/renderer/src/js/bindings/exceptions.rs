//! `DOMException` and its legacy code table.

use super::OptString;

use rquickjs::class::Trace;

/// Legacy `DOMException` constants: name used to derive `code`.
///
/// <https://webidl.spec.whatwg.org/#idl-DOMException>
pub(crate) const DOM_EXCEPTION_CODES: [(&str, i32); 25] = [
    ("INDEX_SIZE_ERR", 1),
    ("DOMSTRING_SIZE_ERR", 2),
    ("HIERARCHY_REQUEST_ERR", 3),
    ("WRONG_DOCUMENT_ERR", 4),
    ("INVALID_CHARACTER_ERR", 5),
    ("NO_DATA_ALLOWED_ERR", 6),
    ("NO_MODIFICATION_ALLOWED_ERR", 7),
    ("NOT_FOUND_ERR", 8),
    ("NOT_SUPPORTED_ERR", 9),
    ("INUSE_ATTRIBUTE_ERR", 10),
    ("INVALID_STATE_ERR", 11),
    ("SYNTAX_ERR", 12),
    ("INVALID_MODIFICATION_ERR", 13),
    ("NAMESPACE_ERR", 14),
    ("INVALID_ACCESS_ERR", 15),
    ("VALIDATION_ERR", 16),
    ("TYPE_MISMATCH_ERR", 17),
    ("SECURITY_ERR", 18),
    ("NETWORK_ERR", 19),
    ("ABORT_ERR", 20),
    ("URL_MISMATCH_ERR", 21),
    ("QUOTA_EXCEEDED_ERR", 22),
    ("TIMEOUT_ERR", 23),
    ("INVALID_NODE_TYPE_ERR", 24),
    ("DATA_CLONE_ERR", 25),
];

/// `DOMException` as a hand-written Rust platform object
/// (<https://webidl.spec.whatwg.org/#idl-DOMException>). DOM operations
/// throw these; a plain `TypeError` would fail `assert_throws_dom`.
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMException")]
pub struct JsDomException {
    pub(crate) name: String,
    pub(crate) message: String,
}

#[rquickjs::methods]
impl JsDomException {
    // https://webidl.spec.whatwg.org/#dom-domexception-domexception
    #[qjs(constructor)]
    fn new(message: OptString, name: OptString) -> Self {
        let name = name.0.filter(|value| !value.is_empty());
        Self {
            name: name.unwrap_or_else(|| "Error".into()),
            message: message.0.unwrap_or_default(),
        }
    }

    #[qjs(get, rename = "name")]
    fn get_name(&self) -> String {
        self.name.clone()
    }

    #[qjs(get, rename = "message")]
    fn get_message(&self) -> String {
        self.message.clone()
    }

    /// Legacy `code`, derived from the name
    /// (<https://webidl.spec.whatwg.org/#dom-domexception-code>).
    #[qjs(get, rename = "code")]
    fn get_code(&self) -> i32 {
        dom_exception_code(&self.name)
    }
}

fn dom_exception_code(name: &str) -> i32 {
    // https://webidl.spec.whatwg.org/#dom-domexception-code: legacy names
    // map to their constant's value; anything else is 0.
    match name {
        "IndexSizeError" => 1,
        "DOMStringSizeError" => 2,
        "HierarchyRequestError" => 3,
        "WrongDocumentError" => 4,
        "InvalidCharacterError" => 5,
        "NoDataAllowedError" => 6,
        "NoModificationAllowedError" => 7,
        "NotFoundError" => 8,
        "NotSupportedError" => 9,
        "InUseAttributeError" => 10,
        "InvalidStateError" => 11,
        "SyntaxError" => 12,
        "InvalidModificationError" => 13,
        "NamespaceError" => 14,
        "InvalidAccessError" => 15,
        "ValidationError" => 16,
        "TypeMismatchError" => 17,
        "SecurityError" => 18,
        "NetworkError" => 19,
        "AbortError" => 20,
        "URLMismatchError" => 21,
        "QuotaExceededError" => 22,
        "TimeoutError" => 23,
        "InvalidNodeTypeError" => 24,
        "DataCloneError" => 25,
        _ => 0,
    }
}
