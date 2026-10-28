//! Enumerated content attributes: one Rust enum per keyword set, with one
//! reflection rule for all of them.
//!
//! `parse` is ASCII case-insensitive and does not trim. Reflection answers the
//! canonical keyword, the missing-value default when the attribute is absent,
//! or the invalid-value default when it is not a keyword
//! (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#limited-to-only-known-values>).

/// One enumerated attribute's keyword set.
///
/// The variant list is the only copy: the macro expands it into the enum, the
/// `KEYWORDS` table reflection reads, and `keyword()`, so a variant cannot be
/// missing from the table or spelled two ways without the code saying so.
macro_rules! enumerated {
    (
        $(#[$doc:meta])*
        $name:ident { $($variant:ident => $keyword:literal),+ $(,)? }
    ) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub(crate) enum $name {
            $($variant),+
        }

        impl Enumerated for $name {
            const KEYWORDS: &'static [Self] = &[$(Self::$variant),+];

            fn keyword(self) -> &'static str {
                match self {
                    $(Self::$variant => $keyword),+
                }
            }
        }
    };
}

/// One enumerated content attribute's keywords.
pub(crate) trait Enumerated: Copy + 'static {
    /// Every keyword, in spec order.
    const KEYWORDS: &'static [Self];

    /// The canonical spelling of the keyword.
    fn keyword(self) -> &'static str;

    /// Parses one keyword, ASCII case-insensitively and untrimmed.
    fn parse(value: &str) -> Option<Self> {
        Self::KEYWORDS
            .iter()
            .copied()
            .find(|candidate| candidate.keyword().eq_ignore_ascii_case(value))
    }
}

/// When an enumerated attribute names no keyword: what its getter answers with
/// the attribute absent, and with the attribute present but invalid.
///
/// The defaults are enum values, not spellings, so a typo cannot compile; the
/// two named fields make an exchange of the spec's missing-value default for
/// its invalid-value default impossible to write by accident.
/// `None` is the spec's "no state", which a getter reports as the empty string
/// (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#limited-to-only-known-values>).
#[derive(Clone, Copy)]
pub(crate) struct Defaults<T> {
    /// The missing-value default.
    pub(crate) missing: Option<T>,
    /// The invalid-value default.
    pub(crate) invalid: Option<T>,
}

impl<T> Defaults<T> {
    /// No default for either state. A `const` so getters that answer the empty
    /// string need not name the enum twice.
    pub(crate) const NONE: Self = Self {
        missing: None,
        invalid: None,
    };

    /// The same default for both states.
    pub(crate) const fn both(value: T) -> Self
    where
        T: Copy,
    {
        Self {
            missing: Some(value),
            invalid: Some(value),
        }
    }
}

/// Enumerated reflection: the canonical keyword, the missing-value default
/// when the attribute is absent, or the invalid-value default when it names no
/// keyword.
pub(crate) fn reflect_enumerated<T: Enumerated>(
    raw: Option<&str>,
    defaults: Defaults<T>,
) -> &'static str {
    let Some(raw) = raw else {
        return defaults.missing.map_or("", |value| value.keyword());
    };
    T::parse(raw)
        .or(defaults.invalid)
        .map_or("", |value| value.keyword())
}

enumerated! {
    /// `dir` (<https://html.spec.whatwg.org/multipage/dom.html#the-dir-attribute>).
    Dir {
        Ltr => "ltr",
        Rtl => "rtl",
        Auto => "auto",
    }
}

enumerated! {
    /// `enterkeyhint`
    /// (<https://html.spec.whatwg.org/multipage/interaction.html#attr-enterkeyhint>).
    EnterKeyHint {
        Enter => "enter",
        Done => "done",
        Go => "go",
        Next => "next",
        Previous => "previous",
        Search => "search",
        Send => "send",
    }
}

enumerated! {
    /// `inputmode`
    /// (<https://html.spec.whatwg.org/multipage/interaction.html#attr-inputmode>).
    InputMode {
        None => "none",
        Text => "text",
        Tel => "tel",
        Url => "url",
        Email => "email",
        Numeric => "numeric",
        Decimal => "decimal",
        Search => "search",
    }
}

enumerated! {
    /// `loading`
    /// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#attr-loading>).
    Loading {
        Lazy => "lazy",
        Eager => "eager",
    }
}

enumerated! {
    /// `preload`
    /// (<https://html.spec.whatwg.org/multipage/media.html#attr-media-preload>).
    Preload {
        None => "none",
        Metadata => "metadata",
        Auto => "auto",
    }
}

enumerated! {
    /// `decoding`
    /// (<https://html.spec.whatwg.org/multipage/embedded-content.html#attr-img-decoding>).
    Decoding {
        Async => "async",
        Sync => "sync",
        Auto => "auto",
    }
}

enumerated! {
    /// `method` and `formmethod`
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fs-method>).
    FormMethod {
        Get => "get",
        Post => "post",
        Dialog => "dialog",
    }
}

enumerated! {
    /// `enctype` and `formenctype`
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fs-enctype>).
    FormEnctype {
        UrlEncoded => "application/x-www-form-urlencoded",
        Multipart => "multipart/form-data",
        TextPlain => "text/plain",
    }
}

enumerated! {
    /// `autocomplete`
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fe-autocomplete>).
    Autocomplete {
        On => "on",
        Off => "off",
    }
}

enumerated! {
    /// `scope` on `th`
    /// (<https://html.spec.whatwg.org/multipage/tables.html#attr-th-scope>).
    TableScope {
        Row => "row",
        Col => "col",
        RowGroup => "rowgroup",
        ColGroup => "colgroup",
    }
}

enumerated! {
    /// `as` on `link`
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#attr-link-as>).
    LinkAs {
        Fetch => "fetch",
        Audio => "audio",
        AudioWorklet => "audioworklet",
        Document => "document",
        Embed => "embed",
        Font => "font",
        Frame => "frame",
        Iframe => "iframe",
        Image => "image",
        Json => "json",
        Manifest => "manifest",
        Object => "object",
        PaintWorklet => "paintworklet",
        Report => "report",
        Script => "script",
        ServiceWorker => "serviceworker",
        SharedWorker => "sharedworker",
        Style => "style",
        Track => "track",
        Video => "video",
        Worker => "worker",
        Xslt => "xslt",
    }
}

enumerated! {
    /// `kind` on `track`
    /// (<https://html.spec.whatwg.org/multipage/media.html#attr-track-kind>).
    TrackKind {
        Subtitles => "subtitles",
        Captions => "captions",
        Descriptions => "descriptions",
        Chapters => "chapters",
        Metadata => "metadata",
    }
}

impl Enumerated for net::cors::CrossOrigin {
    const KEYWORDS: &'static [Self] = net::cors::CrossOrigin::KEYWORDS;

    fn keyword(self) -> &'static str {
        net::cors::CrossOrigin::keyword(self)
    }
}

impl Enumerated for net::ReferrerPolicy {
    const KEYWORDS: &'static [Self] = net::ReferrerPolicy::ALL;

    fn keyword(self) -> &'static str {
        net::ReferrerPolicy::keyword(self)
    }

    /// `net` owns this keyword set, so its parser is the one parser: the
    /// header, the meta form, and reflection cannot drift.
    fn parse(value: &str) -> Option<Self> {
        net::ReferrerPolicy::parse(value)
    }
}

#[cfg(test)]
mod reflection_tests {
    use super::{Defaults, TrackKind, reflect_enumerated};

    /// The four answers a getter can give, from the spec's missing-value and
    /// invalid-value defaults
    /// (<https://html.spec.whatwg.org/multipage/media.html#dom-track-kind>).
    #[test]
    fn reflection_answers_each_state_once() {
        let track = Defaults {
            missing: Some(TrackKind::Subtitles),
            invalid: Some(TrackKind::Metadata),
        };
        assert_eq!(reflect_enumerated::<TrackKind>(None, track), "subtitles");
        assert_eq!(
            reflect_enumerated::<TrackKind>(Some("captions"), track),
            "captions"
        );
        assert_eq!(
            reflect_enumerated::<TrackKind>(Some("CAPTIONS"), track),
            "captions",
            "a keyword matches ASCII case-insensitively"
        );
        assert_eq!(
            reflect_enumerated::<TrackKind>(Some("bogus"), track),
            "metadata",
            "an invalid value reads the invalid-value default"
        );
        assert_eq!(
            reflect_enumerated::<TrackKind>(None, Defaults::NONE),
            "",
            "no missing-value default reads the empty string"
        );
        assert_eq!(
            reflect_enumerated::<TrackKind>(Some("bogus"), Defaults::NONE),
            "",
            "no invalid-value default reads the empty string"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Autocomplete, Decoding, Dir, EnterKeyHint, Enumerated, FormEnctype, FormMethod, InputMode,
        LinkAs, Loading, Preload, TableScope, TrackKind,
    };

    /// Every keyword reflects back to its own value, and no two variants
    /// share a spelling. The macro already generates `KEYWORDS` from the
    /// variant list, so this pins the strings it cannot check.
    fn assert_round_trip<T: Enumerated + PartialEq + std::fmt::Debug>() {
        let mut seen = std::collections::HashSet::new();
        for value in T::KEYWORDS {
            let keyword = value.keyword();
            assert_eq!(T::parse(keyword), Some(*value), "{value:?}");
            assert!(
                T::parse(&keyword.to_ascii_uppercase()) == Some(*value),
                "{value:?} parses case-insensitively"
            );
            assert!(seen.insert(keyword), "{value:?} repeats {keyword:?}");
        }
    }

    #[test]
    fn every_keyword_round_trips_through_reflection() {
        assert_round_trip::<Dir>();
        assert_round_trip::<EnterKeyHint>();
        assert_round_trip::<InputMode>();
        assert_round_trip::<Loading>();
        assert_round_trip::<Preload>();
        assert_round_trip::<Decoding>();
        assert_round_trip::<FormMethod>();
        assert_round_trip::<FormEnctype>();
        assert_round_trip::<Autocomplete>();
        assert_round_trip::<TableScope>();
        assert_round_trip::<LinkAs>();
        assert_round_trip::<TrackKind>();
        assert_round_trip::<net::cors::CrossOrigin>();
        assert_round_trip::<net::ReferrerPolicy>();
    }
}
