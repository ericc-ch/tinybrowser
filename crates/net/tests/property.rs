//! Property tests for net's pure domain modules (decision 10).
//!
//! `HeaderMap` is the first: its two invariants are case-insensitive lookup
//! and insertion-order iteration, both cheap to fuzz and expensive to get
//! subtly wrong.

use proptest::prelude::*;

/// Valid RFC 9110 §5.1 field names with arbitrary ASCII case mixed in.
fn header_name_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z][a-zA-Z0-9_.-]{0,20}").expect("regex strategy compiles")
}

proptest! {
    /// `get` must agree with case-folded lookup for any casing of a stored
    /// name, and iteration must replay exactly what was inserted. Duplicate
    /// names are legal; lookups return the first insert's value.
    #[test]
    fn get_is_case_insensitive_and_iteration_preserves_order(
        names in prop::collection::vec(header_name_strategy(), 1..32),
        seed in proptest::num::u8::ANY,
    ) {
        let mut map = net::HeaderMap::new();
        // Oracle: plain case-fold + first-wins, independent of production's
        // ordered scan.
        let mut oracle: std::collections::HashMap<String, Vec<u8>> =
            std::collections::HashMap::new();
        let mut inserted: Vec<(String, Vec<u8>)> = Vec::new();

        for (i, name) in names.iter().enumerate() {
            // Printable-safe byte: never NUL/CR/LF, which insert rejects.
            let byte = u8::try_from(i % 90 + 33).expect("33..=122 fits u8");
            let value = vec![byte; 3];
            map.insert(name, value.as_slice()).expect("strategy yields valid names");
            oracle.entry(name.to_ascii_lowercase()).or_insert(value.clone());
            inserted.push((name.clone(), value));
        }

        // Iteration replays insertion order verbatim.
        let iterated: Vec<(&str, &[u8])> = map.iter().collect();
        prop_assert_eq!(
            iterated,
            inserted
                .iter()
                .map(|(n, v)| (n.as_str(), v.as_slice()))
                .collect::<Vec<_>>()
        );

        // Every name — re-cased deterministically from the seed — still finds
        // its first-inserted value.
        for (name, _) in &inserted {
            let flipped: String = name
                .chars()
                .enumerate()
                .map(|(i, c)| if (seed >> (i % 8)) & 1 == 1 {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                })
                .collect();
            let expected = oracle
                .get(&name.to_ascii_lowercase())
                .expect("oracle holds every inserted name");
            prop_assert_eq!(map.get(&flipped), Some(expected.as_slice()));
        }
    }
}

fn cookie_token() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z][A-Za-z0-9]{0,12}").expect("regex")
}

proptest! {
    /// Max-Age in the past (≤ 0) never matches; a large positive Max-Age does,
    /// for a same-site document.cookie retrieval on the storing URI.
    #[test]
    fn expired_cookies_stop_matching(
        name in cookie_token(),
        value in cookie_token(),
        max_age in -20i64..20,
    ) {
        let uri = url::Url::parse("https://example.com/app").expect("uri");
        let agent = net::Agent::new();
        agent.set_cookie(
            &format!("{name}={value}; Path=/; Max-Age={max_age}"),
            &uri,
        );
        let got = agent.cookies_for(&uri);
        if max_age <= 0 {
            prop_assert!(!got.contains(&name), "expired cookie leaked: {got}");
        } else {
            prop_assert!(got.contains(&format!("{name}={value}")), "live cookie missing: {got}");
        }
    }

    /// A cookie stored against https://www.example.com/foo either comes back
    /// for that URI or was rejected by domain/path/secure rules — never a
    /// silent half-store.
    #[test]
    fn stored_cookie_is_retrievable_or_was_rejected(
        name in cookie_token(),
        value in cookie_token(),
        domain in prop::sample::select(vec![
            "",
            "example.com",
            "www.example.com",
            "other.com",
            "com",
        ]),
        path in prop::sample::select(vec!["/", "/foo", "/foo/bar", "/zzz"]),
        secure in proptest::bool::ANY,
    ) {
        let uri = url::Url::parse("https://www.example.com/foo/bar").expect("uri");
        let agent = net::Agent::new();
        let mut line = format!("{name}={value}");
        if !domain.is_empty() {
            line.push_str("; Domain=");
            line.push_str(domain);
        }
        line.push_str("; Path=");
        line.push_str(path);
        if secure {
            line.push_str("; Secure");
        }
        agent.set_cookie(&line, &uri);
        let got = agent.cookies_for(&uri);
        let present = got.split("; ").any(|part| part == format!("{name}={value}"));
        // Independent of production matchers: only these Domain/Path pairs
        // are legal for https://www.example.com/foo/bar.
        let should = matches!(
            (domain, path),
            ("" | "example.com" | "www.example.com", "/" | "/foo" | "/foo/bar")
        );
        prop_assert_eq!(present, should, "line={} got={}", line, got);
    }
}
