//! Enumerated content attributes: one Rust enum per keyword set, with one
//! reflection rule for all of them.
//!
//! `parse` is ASCII case-insensitive and does not trim. Reflection answers the
//! canonical keyword, the missing-value default when the attribute is absent,
//! or the invalid-value default when it is not a keyword
//! (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#limited-to-only-known-values>).

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

/// Enumerated reflection: the canonical keyword, `missing` when the attribute
/// is absent, or `invalid` when it names no keyword.
pub(crate) fn reflect_enumerated<T: Enumerated>(
    raw: Option<&str>,
    missing: &str,
    invalid: &str,
) -> String {
    let Some(raw) = raw else {
        return missing.to_owned();
    };
    T::parse(raw).map_or_else(|| invalid.to_owned(), |value| value.keyword().to_owned())
}

/// `dir` (<https://html.spec.whatwg.org/multipage/dom.html#the-dir-attribute>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Dir {
    Ltr,
    Rtl,
    Auto,
}

impl Enumerated for Dir {
    const KEYWORDS: &'static [Self] = &[Self::Ltr, Self::Rtl, Self::Auto];

    fn keyword(self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
            Self::Auto => "auto",
        }
    }
}

/// `enterkeyhint`
/// (<https://html.spec.whatwg.org/multipage/interaction.html#attr-enterkeyhint>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnterKeyHint {
    Enter,
    Done,
    Go,
    Next,
    Previous,
    Search,
    Send,
}

impl Enumerated for EnterKeyHint {
    const KEYWORDS: &'static [Self] = &[
        Self::Enter,
        Self::Done,
        Self::Go,
        Self::Next,
        Self::Previous,
        Self::Search,
        Self::Send,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Done => "done",
            Self::Go => "go",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::Search => "search",
            Self::Send => "send",
        }
    }
}

/// `inputmode`
/// (<https://html.spec.whatwg.org/multipage/interaction.html#attr-inputmode>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputMode {
    None,
    Text,
    Tel,
    Url,
    Email,
    Numeric,
    Decimal,
    Search,
}

impl Enumerated for InputMode {
    const KEYWORDS: &'static [Self] = &[
        Self::None,
        Self::Text,
        Self::Tel,
        Self::Url,
        Self::Email,
        Self::Numeric,
        Self::Decimal,
        Self::Search,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Text => "text",
            Self::Tel => "tel",
            Self::Url => "url",
            Self::Email => "email",
            Self::Numeric => "numeric",
            Self::Decimal => "decimal",
            Self::Search => "search",
        }
    }
}

/// `loading`
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#attr-loading>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Loading {
    Lazy,
    Eager,
}

impl Enumerated for Loading {
    const KEYWORDS: &'static [Self] = &[Self::Lazy, Self::Eager];

    fn keyword(self) -> &'static str {
        match self {
            Self::Lazy => "lazy",
            Self::Eager => "eager",
        }
    }
}

/// `preload`
/// (<https://html.spec.whatwg.org/multipage/media.html#attr-media-preload>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Preload {
    None,
    Metadata,
    Auto,
}

impl Enumerated for Preload {
    const KEYWORDS: &'static [Self] = &[Self::None, Self::Metadata, Self::Auto];

    fn keyword(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Metadata => "metadata",
            Self::Auto => "auto",
        }
    }
}

/// `decoding`
/// (<https://html.spec.whatwg.org/multipage/embedded-content.html#attr-img-decoding>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Decoding {
    Async,
    Sync,
    Auto,
}

impl Enumerated for Decoding {
    const KEYWORDS: &'static [Self] = &[Self::Async, Self::Sync, Self::Auto];

    fn keyword(self) -> &'static str {
        match self {
            Self::Async => "async",
            Self::Sync => "sync",
            Self::Auto => "auto",
        }
    }
}

/// `method` and `formmethod`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fs-method>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormMethod {
    Get,
    Post,
    Dialog,
}

impl Enumerated for FormMethod {
    const KEYWORDS: &'static [Self] = &[Self::Get, Self::Post, Self::Dialog];

    fn keyword(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Dialog => "dialog",
        }
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
        match self {
            Self::UrlEncoded => "application/x-www-form-urlencoded",
            Self::Multipart => "multipart/form-data",
            Self::TextPlain => "text/plain",
        }
    }
}

/// `autocomplete`
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fe-autocomplete>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Autocomplete {
    On,
    Off,
}

impl Enumerated for Autocomplete {
    const KEYWORDS: &'static [Self] = &[Self::On, Self::Off];

    fn keyword(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

/// `scope` on `th`
/// (<https://html.spec.whatwg.org/multipage/tables.html#attr-th-scope>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TableScope {
    Row,
    Col,
    RowGroup,
    ColGroup,
}

impl Enumerated for TableScope {
    const KEYWORDS: &'static [Self] = &[Self::Row, Self::Col, Self::RowGroup, Self::ColGroup];

    fn keyword(self) -> &'static str {
        match self {
            Self::Row => "row",
            Self::Col => "col",
            Self::RowGroup => "rowgroup",
            Self::ColGroup => "colgroup",
        }
    }
}

/// `as` on `link`
/// (<https://html.spec.whatwg.org/multipage/semantics.html#attr-link-as>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinkAs {
    Fetch,
    Audio,
    AudioWorklet,
    Document,
    Embed,
    Font,
    Frame,
    Iframe,
    Image,
    Json,
    Manifest,
    Object,
    PaintWorklet,
    Report,
    Script,
    ServiceWorker,
    SharedWorker,
    Style,
    Track,
    Video,
    Worker,
    Xslt,
}

impl Enumerated for LinkAs {
    const KEYWORDS: &'static [Self] = &[
        Self::Fetch,
        Self::Audio,
        Self::AudioWorklet,
        Self::Document,
        Self::Embed,
        Self::Font,
        Self::Frame,
        Self::Iframe,
        Self::Image,
        Self::Json,
        Self::Manifest,
        Self::Object,
        Self::PaintWorklet,
        Self::Report,
        Self::Script,
        Self::ServiceWorker,
        Self::SharedWorker,
        Self::Style,
        Self::Track,
        Self::Video,
        Self::Worker,
        Self::Xslt,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::Fetch => "fetch",
            Self::Audio => "audio",
            Self::AudioWorklet => "audioworklet",
            Self::Document => "document",
            Self::Embed => "embed",
            Self::Font => "font",
            Self::Frame => "frame",
            Self::Iframe => "iframe",
            Self::Image => "image",
            Self::Json => "json",
            Self::Manifest => "manifest",
            Self::Object => "object",
            Self::PaintWorklet => "paintworklet",
            Self::Report => "report",
            Self::Script => "script",
            Self::ServiceWorker => "serviceworker",
            Self::SharedWorker => "sharedworker",
            Self::Style => "style",
            Self::Track => "track",
            Self::Video => "video",
            Self::Worker => "worker",
            Self::Xslt => "xslt",
        }
    }
}

/// `kind` on `track`
/// (<https://html.spec.whatwg.org/multipage/media.html#attr-track-kind>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrackKind {
    Subtitles,
    Captions,
    Descriptions,
    Chapters,
    Metadata,
}

impl Enumerated for TrackKind {
    const KEYWORDS: &'static [Self] = &[
        Self::Subtitles,
        Self::Captions,
        Self::Descriptions,
        Self::Chapters,
        Self::Metadata,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::Subtitles => "subtitles",
            Self::Captions => "captions",
            Self::Descriptions => "descriptions",
            Self::Chapters => "chapters",
            Self::Metadata => "metadata",
        }
    }
}

/// `crossorigin`
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#attr-crossorigin>).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CrossOrigin {
    Anonymous,
    UseCredentials,
}

impl Enumerated for CrossOrigin {
    const KEYWORDS: &'static [Self] = &[Self::Anonymous, Self::UseCredentials];

    fn keyword(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::UseCredentials => "use-credentials",
        }
    }
}

impl Enumerated for net::ReferrerPolicy {
    const KEYWORDS: &'static [Self] = &net::ReferrerPolicy::ALL;

    fn keyword(self) -> &'static str {
        net::ReferrerPolicy::keyword(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Autocomplete, CrossOrigin, Decoding, Dir, EnterKeyHint, Enumerated, FormEnctype,
        FormMethod, InputMode, LinkAs, Loading, Preload, TableScope, TrackKind,
    };

    /// Every key must parse back to its own value, and the table must list
    /// each one exactly once. A variant missing from `KEYWORDS` is invisible
    /// to reflection but still reachable from `keyword`, so nothing else
    /// catches the split; `expected` is the variant count from the enum
    /// definition.
    fn assert_keyword_table<T: Enumerated + PartialEq + std::fmt::Debug>(expected: usize) {
        assert_eq!(T::KEYWORDS.len(), expected, "keyword count");
        let mut seen = std::collections::HashSet::new();
        for value in T::KEYWORDS {
            assert_eq!(T::parse(value.keyword()), Some(*value), "{value:?}");
            assert!(seen.insert(value.keyword()), "{value:?} listed twice");
        }
    }

    #[test]
    fn keyword_tables_cover_every_variant() {
        assert_keyword_table::<Dir>(3);
        assert_keyword_table::<EnterKeyHint>(7);
        assert_keyword_table::<InputMode>(8);
        assert_keyword_table::<Loading>(2);
        assert_keyword_table::<Preload>(3);
        assert_keyword_table::<Decoding>(3);
        assert_keyword_table::<FormMethod>(3);
        assert_keyword_table::<FormEnctype>(3);
        assert_keyword_table::<Autocomplete>(2);
        assert_keyword_table::<TableScope>(4);
        assert_keyword_table::<LinkAs>(22);
        assert_keyword_table::<TrackKind>(5);
        assert_keyword_table::<CrossOrigin>(2);
        assert_keyword_table::<net::ReferrerPolicy>(8);
    }
}
