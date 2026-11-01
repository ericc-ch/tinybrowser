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
fn parse_boolean(value: &str) -> bool {
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
            .is_some_and(|(_, value)| parse_boolean(value))
    }

    /// What the parsed `noopener` and `noreferrer` features do to the context
    /// this call opens. `noreferrer` implies `noopener`, so the three states
    /// are exhaustive by construction
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#get-noopener-for-window-open>).
    ///
    /// `blob_origin` is the origin of `url`'s blob URL entry, when `url` is a
    /// blob URL this renderer's store knows. Step 1 of "get noopener" severs
    /// the opener for a blob from another site *before* it reads the feature,
    /// against `top_level_origin` rather than the calling frame's origin, so a
    /// same-origin blob opened from a cross-site iframe still severs it. Both
    /// origins being opaque is not same-site: an opaque blob can never be
    /// attributed to the top-level document's site.
    pub(crate) fn opener_policy(
        &self,
        blob_origin: Option<&url::Origin>,
        top_level_origin: &url::Origin,
    ) -> OpenerPolicy {
        let blob_severs =
            blob_origin.is_some_and(|origin| !net::origins_same_site(origin, top_level_origin));
        if self.boolean("noreferrer") {
            OpenerPolicy::NoReferrer
        } else if blob_severs || self.boolean("noopener") {
            OpenerPolicy::NoOpener
        } else {
            OpenerPolicy::Keep
        }
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::WindowFeatures;
    use crate::protocol::OpenerPolicy;

    fn origin(raw: &str) -> url::Origin {
        Url::parse(raw).expect("test URL").origin()
    }

    /// The feature list alone, from a document whose top level is
    /// `https://a.test`.
    fn policy(features: &str) -> OpenerPolicy {
        WindowFeatures::parse(features).opener_policy(None, &origin("https://a.test/"))
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

    /// Step 1 of "get noopener": a blob URL whose entry is from another site
    /// severs the opener even when the feature list says nothing, and even
    /// when `noopener=0` asks to keep it
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#get-noopener-for-window-open>).
    #[test]
    fn a_blob_from_another_site_severs_the_opener() {
        let top = origin("https://a.test/");
        let cross_site = origin("https://b.test/");
        let same_site = origin("https://sub.a.test/");
        assert_eq!(
            WindowFeatures::parse("").opener_policy(Some(&cross_site), &top),
            OpenerPolicy::NoOpener
        );
        assert_eq!(
            WindowFeatures::parse("noopener=0").opener_policy(Some(&cross_site), &top),
            OpenerPolicy::NoOpener
        );
        assert_eq!(
            WindowFeatures::parse("").opener_policy(Some(&same_site), &top),
            OpenerPolicy::Keep
        );
        // The blob rule returns before the feature list is read, so
        // `noreferrer` still keeps its stronger answer.
        assert_eq!(
            WindowFeatures::parse("noreferrer").opener_policy(Some(&cross_site), &top),
            OpenerPolicy::NoReferrer
        );
    }

    /// The blob branch compares against the top-level origin, not the calling
    /// frame's: a blob same-origin with a cross-site iframe still severs the
    /// opener, because the top-level document is what keeps it.
    #[test]
    fn a_blob_must_match_the_top_level_origin() {
        let frame = origin("https://frame.test/");
        let top = origin("https://top.test/");
        assert_eq!(
            WindowFeatures::parse("").opener_policy(Some(&frame), &top),
            OpenerPolicy::NoOpener
        );
        assert_eq!(
            WindowFeatures::parse("").opener_policy(Some(&top), &top),
            OpenerPolicy::Keep
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
