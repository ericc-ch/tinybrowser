use net::{HeaderError, HeaderMap, Method};

#[test]
fn request_tokens_accept_the_protocol_grammar_and_reject_injection() {
    let mut headers = HeaderMap::new();
    let token_alphabet = "!#$%&'*+-.^_`|~09AZaz";
    headers
        .insert(token_alphabet, b"plain\tvalue\x80")
        .expect("full token alphabet and legal value bytes");
    assert_eq!(headers.get(token_alphabet), Some(&b"plain\tvalue\x80"[..]));

    for invalid in [
        "",
        "two words",
        "injected\nX-Forged: 1",
        "\0",
        "\"quoted\"",
        "ünïcode",
    ] {
        assert!(matches!(
            headers.insert(invalid, b"x"),
            Err(HeaderError::InvalidName(name)) if &*name == invalid
        ));
    }
    for invalid in [
        &b"a\0b"[..],
        &b"a\x1fb"[..],
        &b"a\x7fb"[..],
        b"injected\nvalue",
        b"row\r\nX-Forged: yes",
    ] {
        assert!(matches!(
            headers.insert("X-Bad", invalid),
            Err(HeaderError::InvalidValue(value)) if !value.is_empty()
        ));
    }
    assert_eq!(headers.len(), 1);

    for (input, wire) in [
        ("get", "GET"),
        ("Head", "HEAD"),
        ("patch", "patch"),
        ("PATCH", "PATCH"),
        ("propfind", "propfind"),
        ("PropFind", "PropFind"),
    ] {
        assert_eq!(Method::parse(input).expect("valid token").as_str(), wire);
    }
    for invalid in ["", "G ET", "GET\t", "get ", "MËTA"] {
        assert!(
            Method::parse(invalid)
                .expect_err("invalid method")
                .to_string()
                .contains(&format!("{invalid:?}"))
        );
    }
}
