//! The `features` argument of `window.open`, tokenized and read as the
//! window-open steps need it
//! (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#concept-window-open-features-tokenize>).
//!
//! The parser reads the list once for the whole open: the page shim no longer
//! splits it, so there is one grammar and one answer. The comma split the shim
//! used to run and the whitespace split the browser ran disagreed on
//! `noopener=1` and `noopener noreferrer`.
//!
//! Only `noopener` and `noreferrer` are read today, and both are booleans, so
//! the tokenized map is two flags whose last value wins. A feature the window
//! open steps never ask about — a size, `popup` — is tokenized and dropped; add
//! its storage when something reads it.

use crate::protocol::OpenerPolicy;

/// A feature separator: ASCII whitespace, U+003D, or U+002C
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dfn-feature-separator>).
fn is_separator(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == b'=' || byte == b','
}

/// A boolean feature's value: absent or empty is true, `yes` and `true` are
/// true, and anything else is true only when its integer form is non-zero
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#parse-a-boolean-feature>).
///
/// The feature tokenizer lowercases every value, so the words are matched
/// without case. An integer wider than any integer type still parses: one
/// non-zero digit decides it.
fn parse_boolean(value: &str) -> bool {
    if value.is_empty() || value.eq_ignore_ascii_case("yes") || value.eq_ignore_ascii_case("true") {
        return true;
    }
    let digits = value.trim_start_matches(|character: char| character.is_ascii_whitespace());
    let digits = digits.strip_prefix(['+', '-']).unwrap_or(digits);
    digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .any(|digit| digit != b'0')
}

/// The `noopener` and `noreferrer` features, keeping the last value a repeated
/// name gives them
/// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#tokenize-the-features-argument>).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WindowFeatures {
    noopener: bool,
    noreferrer: bool,
}

impl WindowFeatures {
    /// Tokenizes one `features` argument
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#tokenize-the-features-argument>).
    pub(crate) fn parse(features: &str) -> Self {
        let mut parsed = Self::default();
        // Every separator is ASCII, so byte positions are character
        // boundaries and the slices below are always valid UTF-8.
        let bytes = features.as_bytes();
        let mut position = 0;
        while position < bytes.len() {
            while position < bytes.len() && is_separator(bytes[position]) {
                position += 1;
            }
            let name_start = position;
            while position < bytes.len() && !is_separator(bytes[position]) {
                position += 1;
            }
            let name = &features[name_start..position];
            // Skip to the first `=` but never past a `,` or a name character.
            while position < bytes.len() && bytes[position] != b'=' {
                if bytes[position] == b',' || !is_separator(bytes[position]) {
                    break;
                }
                position += 1;
            }
            let mut value = "";
            if position < bytes.len() && is_separator(bytes[position]) {
                while position < bytes.len() && is_separator(bytes[position]) {
                    if bytes[position] == b',' {
                        break;
                    }
                    position += 1;
                }
                let value_start = position;
                while position < bytes.len() && !is_separator(bytes[position]) {
                    position += 1;
                }
                value = &features[value_start..position];
            }
            if name.is_empty() {
                continue;
            }
            if name.eq_ignore_ascii_case("noopener") {
                parsed.noopener = parse_boolean(value);
            } else if name.eq_ignore_ascii_case("noreferrer") {
                parsed.noreferrer = parse_boolean(value);
            }
        }
        parsed
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
        self,
        blob_origin: Option<&url::Origin>,
        top_level_origin: &url::Origin,
    ) -> OpenerPolicy {
        let blob_severs =
            blob_origin.is_some_and(|origin| !net::origins_same_site(origin, top_level_origin));
        if self.noreferrer {
            OpenerPolicy::NoReferrer
        } else if blob_severs || self.noopener {
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
        assert_eq!(policy("noopener=00x1"), OpenerPolicy::Keep);
        assert_eq!(policy("noopener=x1"), OpenerPolicy::Keep);
        assert_eq!(policy("noopener=-5"), OpenerPolicy::NoOpener);
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
}
