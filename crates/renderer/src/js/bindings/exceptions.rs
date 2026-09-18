//! `DOMException` and its legacy code table.

use super::OptString;

use rquickjs::class::Trace;
use rquickjs::{Ctx, Value};

/// Legacy `DOMException` constants: constant suffix, public name, and code.
///
/// <https://webidl.spec.whatwg.org/#idl-DOMException>
pub(crate) const DOM_EXCEPTION_CODES: [(&str, &str, i32); 25] = [
    ("INDEX_SIZE_ERR", "IndexSizeError", 1),
    ("DOMSTRING_SIZE_ERR", "DOMStringSizeError", 2),
    ("HIERARCHY_REQUEST_ERR", "HierarchyRequestError", 3),
    ("WRONG_DOCUMENT_ERR", "WrongDocumentError", 4),
    ("INVALID_CHARACTER_ERR", "InvalidCharacterError", 5),
    ("NO_DATA_ALLOWED_ERR", "NoDataAllowedError", 6),
    (
        "NO_MODIFICATION_ALLOWED_ERR",
        "NoModificationAllowedError",
        7,
    ),
    ("NOT_FOUND_ERR", "NotFoundError", 8),
    ("NOT_SUPPORTED_ERR", "NotSupportedError", 9),
    ("INUSE_ATTRIBUTE_ERR", "InUseAttributeError", 10),
    ("INVALID_STATE_ERR", "InvalidStateError", 11),
    ("SYNTAX_ERR", "SyntaxError", 12),
    ("INVALID_MODIFICATION_ERR", "InvalidModificationError", 13),
    ("NAMESPACE_ERR", "NamespaceError", 14),
    ("INVALID_ACCESS_ERR", "InvalidAccessError", 15),
    ("VALIDATION_ERR", "ValidationError", 16),
    ("TYPE_MISMATCH_ERR", "TypeMismatchError", 17),
    ("SECURITY_ERR", "SecurityError", 18),
    ("NETWORK_ERR", "NetworkError", 19),
    ("ABORT_ERR", "AbortError", 20),
    ("URL_MISMATCH_ERR", "URLMismatchError", 21),
    ("QUOTA_EXCEEDED_ERR", "QuotaExceededError", 22),
    ("TIMEOUT_ERR", "TimeoutError", 23),
    ("INVALID_NODE_TYPE_ERR", "InvalidNodeTypeError", 24),
    ("DATA_CLONE_ERR", "DataCloneError", 25),
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

    /// `QuotaExceededError.requested`; this user agent does not name a
    /// requested size, so the value is `null` for that name and absent
    /// (undefined) for every other exception
    /// (<https://storage.spec.whatwg.org/#quotaexceedederror>).
    #[qjs(get, rename = "requested")]
    fn get_requested<'js>(&self, ctx: Ctx<'js>) -> Value<'js> {
        if self.name == "QuotaExceededError" {
            Value::new_null(ctx)
        } else {
            Value::new_undefined(ctx)
        }
    }

    /// `QuotaExceededError.quota`; see [`Self::get_requested`].
    #[qjs(get, rename = "quota")]
    fn get_quota<'js>(&self, ctx: Ctx<'js>) -> Value<'js> {
        if self.name == "QuotaExceededError" {
            Value::new_null(ctx)
        } else {
            Value::new_undefined(ctx)
        }
    }
}

fn dom_exception_code(name: &str) -> i32 {
    // https://webidl.spec.whatwg.org/#dom-domexception-code: legacy names
    // map to their constant's value; anything else is 0.
    DOM_EXCEPTION_CODES
        .iter()
        .find(|&&(_, public, _)| public == name)
        .map_or(0, |&(_, _, code)| code)
}
