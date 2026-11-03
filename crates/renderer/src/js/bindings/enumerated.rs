//! Enumerated content attributes: one Rust enum per keyword set, with one
//! reflection rule for all of them.
//!
//! `parse` is ASCII case-insensitive and does not trim. Reflection answers the
//! canonical keyword, the missing-value default when the attribute is absent,
//! or the invalid-value default when it is not a keyword
//! (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#limited-to-only-known-values>).
//!
//! The declaration form is `enumerated::enumerated!`, shared with `net`'s
//! `ReferrerPolicy` and `CrossOrigin`.

pub(crate) use enumerated::Enumerated;
use enumerated::enumerated;

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
    pub(crate) Dir {
        Ltr => "ltr",
        Rtl => "rtl",
        Auto => "auto",
    }
}

enumerated! {
    /// `enterkeyhint`
    /// (<https://html.spec.whatwg.org/multipage/interaction.html#attr-enterkeyhint>).
    pub(crate) EnterKeyHint {
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
    pub(crate) InputMode {
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
    pub(crate) Loading {
        Lazy => "lazy",
        Eager => "eager",
    }
}

enumerated! {
    /// `preload`
    /// (<https://html.spec.whatwg.org/multipage/media.html#attr-media-preload>).
    pub(crate) Preload {
        None => "none",
        Metadata => "metadata",
        Auto => "auto",
    }
}

enumerated! {
    /// `decoding`
    /// (<https://html.spec.whatwg.org/multipage/embedded-content.html#attr-img-decoding>).
    pub(crate) Decoding {
        Async => "async",
        Sync => "sync",
        Auto => "auto",
    }
}

/// `method` and `formmethod`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fs-method>).
///
/// The parser is the engine's form-submission parser, so the reflection getter
/// and the submission plan cannot disagree about a value such as `" post"`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormMethod {
    Get,
    Post,
    Dialog,
}

impl Enumerated for FormMethod {
    const KEYWORDS: &'static [Self] = &[Self::Get, Self::Post, Self::Dialog];

    fn keyword(self) -> &'static str {
        let engine = match self {
            Self::Get => blitz_dom::form::FormMethod::Get,
            Self::Post => blitz_dom::form::FormMethod::Post,
            Self::Dialog => blitz_dom::form::FormMethod::Dialog,
        };
        engine.as_str()
    }

    fn parse(value: &str) -> Option<Self> {
        use std::str::FromStr;
        Some(match blitz_dom::form::FormMethod::from_str(value).ok()? {
            blitz_dom::form::FormMethod::Get => Self::Get,
            blitz_dom::form::FormMethod::Post => Self::Post,
            blitz_dom::form::FormMethod::Dialog => Self::Dialog,
        })
    }
}

/// `enctype` and `formenctype`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fs-enctype>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormEnctype {
    UrlEncoded,
    Multipart,
    TextPlain,
}

impl Enumerated for FormEnctype {
    const KEYWORDS: &'static [Self] = &[Self::UrlEncoded, Self::Multipart, Self::TextPlain];

    fn keyword(self) -> &'static str {
        let engine = match self {
            Self::UrlEncoded => blitz_dom::form::RequestContentType::FormUrlEncoded,
            Self::Multipart => blitz_dom::form::RequestContentType::MultipartFormData,
            Self::TextPlain => blitz_dom::form::RequestContentType::TextPlain,
        };
        engine.as_str()
    }

    fn parse(value: &str) -> Option<Self> {
        use std::str::FromStr;
        Some(
            match blitz_dom::form::RequestContentType::from_str(value).ok()? {
                blitz_dom::form::RequestContentType::FormUrlEncoded => Self::UrlEncoded,
                blitz_dom::form::RequestContentType::MultipartFormData => Self::Multipart,
                blitz_dom::form::RequestContentType::TextPlain => Self::TextPlain,
            },
        )
    }
}

enumerated! {
    /// `autocomplete`
    /// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fe-autocomplete>).
    pub(crate) Autocomplete {
        On => "on",
        Off => "off",
    }
}

enumerated! {
    /// `scope` on `th`
    /// (<https://html.spec.whatwg.org/multipage/tables.html#attr-th-scope>).
    pub(crate) TableScope {
        Row => "row",
        Col => "col",
        RowGroup => "rowgroup",
        ColGroup => "colgroup",
    }
}

enumerated! {
    /// `as` on `link`
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#attr-link-as>).
    pub(crate) LinkAs {
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
    pub(crate) TrackKind {
        Subtitles => "subtitles",
        Captions => "captions",
        Descriptions => "descriptions",
        Chapters => "chapters",
        Metadata => "metadata",
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
