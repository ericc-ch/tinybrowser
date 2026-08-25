//! Boundary grammars: header names/values and method tokens.
//!
//! The negative paths of net's pure boundary parsers (RFC 9110 §5.1
//! field-name, §5.5 field-value, §9.1 `method = token`, WHATWG fetch
//! #methods normalization). The property test's happy-path strategy never
//! generates these; injection-shaped garbage must be pinned explicitly.

use net::{HeaderError, HeaderMap, Method};

#[test]
fn header_names_outside_the_token_grammar_are_rejected() {
    let mut map = HeaderMap::new();
    for bad in [
        "",                      // empty
        "two words",             // SP is not a tchar
        "trailing ",             // not even at the edge
        "injected\nX-Forged: 1", // LF guard
        "\u{0}",                 // NUL
        "\"quoted\"",            // DQUOTE is not a tchar
        "ünïcode",               // non-ASCII
    ] {
        match map.insert(bad, b"x") {
            Err(HeaderError::InvalidName(name)) => assert_eq!(&*name, bad),
            other => panic!("insert({bad:?}) should reject the name, got {other:?}"),
        }
    }
}

#[test]
fn header_names_accept_the_full_token_alphabet() {
    let mut map = HeaderMap::new();
    // Every RFC 9110 §5.1 tchar outside alphanumerics, plus digits/case.
    let tricky = "!#$%&'*+-.^_`|~09AZaz";
    map.insert(tricky, b"v")
        .expect("all token characters are legal names");
    assert_eq!(map.get(tricky), Some(&b"v"[..]));

    // Lookups are infallible by design: an invalid token matches nothing.
    assert_eq!(map.get(""), None);
    assert_eq!(map.get("no spaces"), None);
}

#[test]
fn header_values_allow_htab_and_obs_text_reject_other_controls() {
    let mut map = HeaderMap::new();
    // RFC 9110 §5.5 field-value: VCHAR / SP / HTAB / obs-text.
    map.insert("X-Sp", b"a plain value").expect("SP is legal");
    map.insert("X-Htab", b"col\tumn").expect("HTAB is legal");
    map.insert("X-ObsText", b"caf\xc3\xa9")
        .expect("obs-text bytes are legal");

    for bad in [
        &b"a\x00b"[..],          // NUL
        &b"a\x1Fb"[..],          // unit-separator CTL
        &b"a\x7Fb"[..],          // DEL
        b"injected\nvalue",      // bare LF
        b"row\r\nX-Forged: yes", // CRLF injection guard
    ] {
        match map.insert("X-Bad", bad) {
            Err(HeaderError::InvalidValue(value)) => assert!(!value.is_empty()),
            other => panic!("insert({bad:?}) should reject the value, got {other:?}"),
        }
    }

    // Rejected values leave no partial state behind.
    assert_eq!(map.len(), 3);
}

#[test]
fn method_tokens_outside_the_token_grammar_are_rejected() {
    for bad in ["", "G ET", "GET\t", "get ", "MËTA"] {
        match Method::parse(bad) {
            Err(err) => {
                // Display carries the offending token verbatim, rendered
                // through Debug (so e.g. tabs surface as \t escapes).
                assert!(
                    err.to_string().contains(&format!("{bad:?}")),
                    "{bad:?} must surface in the rejection"
                );
            }
            Ok(method) => panic!("parse({bad:?}) should reject, got {method:?}"),
        }
    }
}

#[test]
fn parsed_methods_equality_follows_wire_tokens() {
    // Any casing of the six normalize-list methods equals its constant
    // (fetch #methods).
    assert_eq!(Method::parse("get").expect("valid"), Method::GET);
    assert_eq!(Method::parse("Head").expect("valid"), Method::HEAD);
    assert_eq!(Method::parse("PATCH").expect("valid"), Method::PATCH);
    // PATCH sits outside that list on purpose: lowercase input is a
    // different wire token, so it must not compare equal.
    assert_ne!(Method::parse("patch").expect("valid"), Method::PATCH);
    // Extension tokens are stored verbatim; casing distinguishes them.
    assert_eq!(
        Method::parse("propfind").expect("valid"),
        Method::parse("propfind").expect("valid")
    );
    assert_ne!(
        Method::parse("propfind").expect("valid"),
        Method::parse("PropFind").expect("valid")
    );
}
