//! The `features` argument of `window.open`, tokenized and read as the
//! window-open steps need it
//! (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#concept-window-open-features-tokenize>).
//!
//! One parser here replaces the page shim's own comma split and the browser's
//! whitespace split, which disagreed: `noopener=1` was true in the shim and
//! false in the browser, and `noopener noreferrer` was the other way round.

use crate::protocol::OpenerPolicy;

/// A feature separator: ASCII whitespace, U+003D, or U+002C
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dfn-feature-separator>).
fn is_separator(character: char) -> bool {
    character.is_ascii_whitespace() || character == '=' || character == ','
}

/// One feature name, normalized to the names the platform reads: `left`,
/// `top`, `width`, and `height`
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#normalize-the-feature-name>).
fn normalize(name: &str) -> &str {
    match name {
        "screenx" => "left",
        "screeny" => "top",
        "innerwidth" => "width",
        "innerheight" => "height",
        other => other,
    }
}

/// A boolean feature's value: absent or empty is true, `yes` and `true` are
/// true, and anything else is true only when its integer form is non-zero
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#parse-a-boolean-feature>).
fn boolean(value: &str) -> bool {
    match value {
        "" | "yes" | "true" => true,
        other => {
            let rest = other.trim_start_matches(|character: char| character.is_ascii_whitespace());
            let digits = rest
                .strip_prefix(['+', '-'])
                .unwrap_or(rest)
                .trim_start_matches(|character: char| character.is_ascii_whitespace());
            let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
            // Any non-zero digit makes the integer non-zero, whatever its
            // width, so a value too long for an integer type still parses.
            digits.bytes().any(|digit| digit != b'0')
        }
    }
}

/// The tokenized `features` argument: every `name=value` pair the caller gave,
/// with a repeated name keeping its first position and its last value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WindowFeatures(Vec<(String, String)>);

impl WindowFeatures {
    /// Tokenizes one `features` argument
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#tokenize-the-features-argument>).
    pub(crate) fn parse(features: &str) -> Self {
        let characters: Vec<char> = features.chars().collect();
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut position = 0;
        while position < characters.len() {
            while position < characters.len() && is_separator(characters[position]) {
                position += 1;
            }
            let mut name = String::new();
            while position < characters.len() && !is_separator(characters[position]) {
                name.push(characters[position].to_ascii_lowercase());
                position += 1;
            }
            let name = normalize(&name).to_owned();
            // Skip to the first `=` but never past a `,` or a name character.
            while position < characters.len() && characters[position] != '=' {
                if characters[position] == ',' || !is_separator(characters[position]) {
                    break;
                }
                position += 1;
            }
            let mut value = String::new();
            if position < characters.len() && is_separator(characters[position]) {
                while position < characters.len() && is_separator(characters[position]) {
                    if characters[position] == ',' {
                        break;
                    }
                    position += 1;
                }
                while position < characters.len() && !is_separator(characters[position]) {
                    value.push(characters[position].to_ascii_lowercase());
                    position += 1;
                }
            }
            if name.is_empty() {
                continue;
            }
            match pairs.iter_mut().find(|(existing, _)| *existing == name) {
                Some(pair) => pair.1 = value,
                None => pairs.push((name, value)),
            }
        }
        Self(pairs)
    }

    /// Whether the argument names `name` as a true boolean feature.
    fn boolean(&self, name: &str) -> bool {
        self.0
            .iter()
            .find(|(existing, _)| existing == name)
            .is_some_and(|(_, value)| boolean(value))
    }

    /// What the parsed `noopener` and `noreferrer` features do to the context
    /// this call opens. `noreferrer` implies `noopener`, so the three states
    /// are exhaustive by construction
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#get-noopener-for-window-open>).
    pub(crate) fn opener_policy(&self) -> OpenerPolicy {
        if self.boolean("noreferrer") {
            OpenerPolicy::NoReferrer
        } else if self.boolean("noopener") {
            OpenerPolicy::NoOpener
        } else {
            OpenerPolicy::Keep
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WindowFeatures;
    use crate::protocol::OpenerPolicy;

    fn policy(features: &str) -> OpenerPolicy {
        WindowFeatures::parse(features).opener_policy()
    }

    /// The four disagreements between the shim's comma split and the browser's
    /// whitespace split, plus the tokenizer's own cases
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#concept-window-open-features-tokenize>).
    #[test]
    fn a_window_feature_list_reads_the_way_the_standard_says() {
        assert_eq!(policy(""), OpenerPolicy::Keep);
        assert_eq!(policy("width=100,height=200"), OpenerPolicy::Keep);
        assert_eq!(policy("noopener"), OpenerPolicy::NoOpener);
        assert_eq!(policy("NOOPENER"), OpenerPolicy::NoOpener);
        assert_eq!(policy("noopener=1"), OpenerPolicy::NoOpener);
        assert_eq!(policy("noopener=yes"), OpenerPolicy::NoOpener);
        assert_eq!(policy("noopener=0"), OpenerPolicy::Keep);
        assert_eq!(policy("noopener=false"), OpenerPolicy::Keep);
        assert_eq!(policy("noopener noreferrer"), OpenerPolicy::NoReferrer);
        assert_eq!(policy("width=100 noopener"), OpenerPolicy::NoOpener);
        assert_eq!(policy("popup,noopener=1"), OpenerPolicy::NoOpener);
        assert_eq!(policy("noreferrer=0"), OpenerPolicy::Keep);
        assert_eq!(policy("noopener=0,noreferrer"), OpenerPolicy::NoReferrer);
        assert_eq!(
            policy("noopener=0,noopener=1"),
            OpenerPolicy::NoOpener,
            "a repeated feature keeps its last value"
        );
    }

    /// A name alias maps to the feature the platform reads, so `screenX` and
    /// `left` are one entry
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#normalize-the-feature-name>).
    #[test]
    fn a_feature_alias_is_the_same_entry() {
        let features = WindowFeatures::parse("screenX=10,left=20");
        assert_eq!(features, WindowFeatures(vec![("left".into(), "20".into())]));
    }
}
