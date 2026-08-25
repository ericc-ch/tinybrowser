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
