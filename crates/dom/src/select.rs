//! Selector matching over the arena, powered by Servo's `selectors` engine.
//!
//! Search takes a CSS selector string, compiles it against our name types,
//! and walks descendants in document order, `querySelector` /
//! `querySelectorAll` semantics, minus boxes: known pseudo-elements parse
//! like browsers' and match nothing (`qSA("p::before")` returns an empty
//! list in every browser: MDN, "querySelectorAll"); unknown ones are
//! refused, also as browsers refuse them.
//!
//! State pseudo-classes are defined here, not by the engine: `selectors`
//! owns selector *grammar* and tree-structural states (`:nth-child`,
//! `:empty`, …) and delegates named states to the embedder through two
//! parse hooks whose results come back to
//! [`Element::match_non_ts_pseudo_class`]. Our answers live in
//! [`crate::state`] under one truth policy: a state matches when static
//! markup determines it, misses vacuously when its context cannot exist in
//! a headless tree (`:hover`, `:visited`, …), and anything outside both
//! categories is refused at parse time, exactly as browsers refuse unknown
//! names. A few states browsers *do* know (`:valid`/`:invalid`,
//! `:open`/`:modal`, `:popover-open`) belong to neither bucket yet; they
//! measure models this tree does not have (constraint validation, dialog
//! state) and stay refused until those models exist; the audit trail lists
//! them rather than silently answering.
//!
//! Selector *names* are newtypes over the same interned atoms elements carry
//! ([`crate::QualName`]), so a compiled selector compares names without ever
//! materializing strings.

use std::borrow::Borrow;
use std::fmt;

use cssparser::{BasicParseErrorKind, Parser as CssParser, ParserInput, ToCss, Token};
use precomputed_hash::PrecomputedHash;
use selectors::{
    Element, OpaqueElement,
    attr::{AttrSelectorOperation, NamespaceConstraint},
    context::{
        MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags,
        QuirksMode as EngineQuirksMode, SelectorCaches,
    },
    matching::matches_selector_list,
    parser::{
        NonTSPseudoClass as NonTSPseudoClassTrait, ParseRelative, Parser as SelectorParser,
        PseudoElement as PseudoElementTrait, SelectorImpl, SelectorList, SelectorParseErrorKind,
    },
};

use crate::arena::{Dom, QuirksMode};
use crate::id::NodeId;
use crate::node::{Attribute, NodeKind};
use crate::state;

impl QuirksMode {
    /// The engine's spelling of this mode.
    fn engine(self) -> EngineQuirksMode {
        match self {
            QuirksMode::NoQuirks => EngineQuirksMode::NoQuirks,
            QuirksMode::LimitedQuirks => EngineQuirksMode::LimitedQuirks,
            QuirksMode::Quirks => EngineQuirksMode::Quirks,
        }
    }
}

/// Why a selector search failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectError {
    /// The scope or element handle named a node that no longer exists.
    StaleNode,
    /// [`Dom::matches`] was handed a handle that does not name an element.
    NotAnElement,
    /// The selector string did not parse.
    Syntax(ParseFail),
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleNode => f.write_str("stale node handle"),
            Self::NotAnElement => f.write_str("handle does not name an element"),
            Self::Syntax(why) => write!(f, "invalid selector: {why}"),
        }
    }
}

impl std::error::Error for SelectError {}

// ── Name types ──────────────────────────────────────────────────────────────
//
// The engine is generic over a `SelectorImpl`; these are ours. Each wraps a
// `markup5ever` atom so parsed selectors share the tree's interning tables.
// The wrappers exist only because the engine demands `cssparser::ToCss` and
// `Borrow<str>`, which the foreign atom types cannot be given from here.

macro_rules! atom_wrapper {
    ($(#[$doc:meta])* $name:ident($inner:ty)) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Eq, PartialEq, Hash)]
        struct $name($inner);

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(<$inner>::from(s))
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl ToCss for $name {
            fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
                dest.write_str(&self.0)
            }
        }

        impl PrecomputedHash for $name {
            fn precomputed_hash(&self) -> u32 {
                self.0.precomputed_hash()
            }
        }
    };
}

atom_wrapper!(
    /// A class or id identifier (`#nav`, `.item`).
    #[derive(Default)]
    Ident(markup5ever::LocalName)
);
atom_wrapper!(
    /// An element's local name (`p`, `circle`, any namespace).
    #[derive(Default)]
    TagLocalName(markup5ever::LocalName)
);
atom_wrapper!(
    /// A namespace URL in an explicit-namespace selector.
    #[derive(Default)]
    NsUrl(markup5ever::Namespace)
);
atom_wrapper!(
    /// A namespace prefix (`svg|circle`): never resolved; see [`SelectorLanguage`].
    #[derive(Default)]
    NsPrefix(markup5ever::Prefix)
);

/// An attribute value on the right-hand side of `[attr=value]`.
#[derive(Clone, Debug, Eq, PartialEq)]
struct AttrValue(String);

impl From<&str> for AttrValue {
    fn from(s: &str) -> Self {
        Self(s.into())
    }
}

impl AsRef<str> for AttrValue {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl ToCss for AttrValue {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        // Values are echoed only when serializing selectors back to CSS,
        // which dom v1 never does; quoting keeps the output honest anyway.
        write!(
            dest,
            "\"{}\"",
            self.0.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }
}

impl PrecomputedHash for AttrValue {
    // DJB2 (Daniel J. Bernstein): seed 5381, wrap-multiply by 33, add byte,
    // the same constant recipe markup5ever's atoms use, so both sides of a
    // comparison hash alike.
    fn precomputed_hash(&self) -> u32 {
        self.0.bytes().fold(5381_u32, |hash, byte| {
            hash.wrapping_mul(33).wrapping_add(u32::from(byte))
        })
    }
}

/// Named element states, parsed from selector text and answered against the
/// tree by [`crate::state`]. Grouped by defining spec section; the vacuous
/// set is explicit so "matches nothing" reads as a decision, not an omission.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PseudoClass {
    // Hyperlinks (Selectors 4 §:link/:any-link; SVG2 for svg <a>).
    /// `:link` / `:any-link`, a hyperlink: `a`/`area`/`link` with `href`.
    AnyLink,
    /// `:link`: same hyperlink rule as [`PseudoClass::AnyLink`]; history
    /// would only split them for `:visited`, which never matches here.
    Link,
    // Form-control UI states (HTML §pseudo-classes).
    Enabled,
    Disabled,
    Checked,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    PlaceholderShown,
    /// `:default`, static subset: default checkedness/selectedness true
    /// (`checked` checkbox/radio inputs, `selected` options); see
    /// [`crate::state::is_default`] for the deferred clause.
    Default,
    /// `:indeterminate`, static subset: a `progress` without a value
    /// attribute; radio groups await the forms model.
    Indeterminate,
    /// `:defined`: true except for valid-but-unregistered custom-element
    /// names (HTML names containing `-`, minus the reserved set).
    Defined,
    // Inherited document-language states (Selectors 4).
    /// `:lang(range…)`: nearest inherited `lang`, matched against each
    /// comma-separated range under RFC 4647 extended filtering.
    Lang(Vec<Box<str>>),
    /// `:dir(direction)`: nearest inherited `dir`, `ltr` default.
    Dir(Box<str>),
    // Vacuous states: real truth needs runtime context a headless tree
    // cannot have. They parse (browsers never throw on these) and match
    // nothing, exactly what a fresh page answers in a live browser.
    /// `:visited`: no browsing history exists.
    Visited,
    /// `:hover`: there is no pointer.
    Hover,
    /// `:active`: nothing is being pressed.
    Active,
    /// `:focus`: no focus owner until the js layer can hold one.
    Focus,
    /// `:focus-within`: no focus owner to be inside of.
    FocusWithin,
    /// `:focus-visible`: no focus heuristics without input events.
    FocusVisible,
    /// `:target`: no URL fragment is in play during a query.
    Target,
    /// `:in-range` / `:out-of-range`: need a live value and min/max model;
    /// statically knowable only once numbers are parsed out of values, so
    /// deferred whole (subagent review R3-10).
    InRange,
    OutOfRange,
    /// `:autofill`: autofill is a live UA activity, not markup.
    Autofill,
}

impl PseudoClass {
    /// Parses one non-functional pseudo-class keyword (CSS keywords are
    /// case-insensitive). Functional ones (`:lang()`, `:dir()`) arrive
    /// through the parser's functional hook below.
    fn from_keyword(name: &str) -> Option<Self> {
        const KEYWORDS: &[(&str, PseudoClass)] = &[
            ("any-link", PseudoClass::AnyLink),
            ("link", PseudoClass::Link),
            ("enabled", PseudoClass::Enabled),
            ("disabled", PseudoClass::Disabled),
            ("checked", PseudoClass::Checked),
            ("required", PseudoClass::Required),
            ("optional", PseudoClass::Optional),
            ("read-only", PseudoClass::ReadOnly),
            ("read-write", PseudoClass::ReadWrite),
            ("placeholder-shown", PseudoClass::PlaceholderShown),
            ("defined", PseudoClass::Defined),
            ("visited", PseudoClass::Visited),
            ("hover", PseudoClass::Hover),
            ("active", PseudoClass::Active),
            ("focus", PseudoClass::Focus),
            ("focus-within", PseudoClass::FocusWithin),
            ("focus-visible", PseudoClass::FocusVisible),
            ("target", PseudoClass::Target),
            ("indeterminate", PseudoClass::Indeterminate),
            ("default", PseudoClass::Default),
            ("in-range", PseudoClass::InRange),
            ("out-of-range", PseudoClass::OutOfRange),
            ("autofill", PseudoClass::Autofill),
        ];
        KEYWORDS
            .iter()
            .find(|(keyword, _)| name.eq_ignore_ascii_case(keyword))
            .map(|(_, class)| class.clone())
    }

    /// Parses one functional pseudo-class argument out of an already-opened
    /// argument block (`(` consumed by the engine, closing delimiter
    /// invisible to us: the nested parser reports end-of-input there).
    ///
    /// Whitespace and comments inside the block are insignificant:
    /// `:lang( en )` parses like `:lang(en)` (Selectors 4 §whitespace).
    fn parse_functional<'i>(
        name: &str,
        input: &mut CssParser<'i, '_>,
    ) -> Result<Self, cssparser::ParseError<'i, ParseFail>> {
        // One bare argument: an identifier, a quoted string, or a lone `*`
        // wildcard (compound wildcards like `*-Cyrl` must be quoted; CSS
        // lexes an unquoted `*` as its own token, so the pieces would
        // arrive separately). Anything else is grammar misuse.
        let take_argument =
            |input: &mut CssParser<'i, '_>| -> Result<String, cssparser::ParseError<'i, ParseFail>> {
                input.skip_whitespace();
                let token = input.next()?.clone();
                match token {
                    Token::Ident(ref value) | Token::QuotedString(ref value) => {
                        Ok(value.to_string())
                    }
                    Token::Delim('*') => Ok("*".into()),
                    unexpected => Err(input
                        .new_basic_error(BasicParseErrorKind::UnexpectedToken(unexpected))
                        .into()),
                }
            };
        if name.eq_ignore_ascii_case("lang") {
            // Comma-separated range list: Selectors 4's own example is
            // `E:lang(sr, "*-Cyrl")`. Each range must be non-empty; empty
            // ranges match nothing under extended filtering, so accepting
            // them would only manufacture silent dead selectors.
            let mut ranges = Vec::new();
            loop {
                let range = take_argument(input)?;
                if range.is_empty() {
                    return Err(input.new_custom_error(
                        SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                            ":lang(\"\")".into(),
                        ),
                    ));
                }
                ranges.push(range.into());
                input.skip_whitespace();
                match input.next() {
                    // Clean end of the argument block: list complete.
                    Err(_) => break,
                    // A comma means another range follows.
                    Ok(&Token::Comma) => {}
                    // Anything else is junk after a complete range.
                    Ok(unexpected) => {
                        let unexpected = unexpected.clone();
                        return Err(input
                            .new_basic_error(BasicParseErrorKind::UnexpectedToken(unexpected))
                            .into());
                    }
                }
            }
            Ok(Self::Lang(ranges))
        } else if name.eq_ignore_ascii_case("dir") {
            let value = take_argument(input)?;
            // Engines refuse any other direction outright (`:dir(up)` →
            // SyntaxError in Chrome and Firefox alike); values that merely
            // fail to *match* (like `auto`) are not parse errors at all.
            // This refusal mirrors engine behavior, not §dir-pseudo's
            // grammar, which would accept-and-never-match.
            if value.eq_ignore_ascii_case("ltr") || value.eq_ignore_ascii_case("rtl") {
                Ok(Self::Dir(value.to_ascii_lowercase().into()))
            } else {
                Err(input.new_custom_error(
                    SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                        format!(":dir({value})").into(),
                    ),
                ))
            }
        } else {
            Err(
                input.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                    name.into(),
                )),
            )
        }
    }

    /// The canonical lowercase keyword, for CSS serialization.
    fn keyword(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::AnyLink => "any-link".into(),
            Self::Link => "link".into(),
            Self::Enabled => "enabled".into(),
            Self::Disabled => "disabled".into(),
            Self::Checked => "checked".into(),
            Self::Required => "required".into(),
            Self::Optional => "optional".into(),
            Self::ReadOnly => "read-only".into(),
            Self::ReadWrite => "read-write".into(),
            Self::PlaceholderShown => "placeholder-shown".into(),
            Self::Defined => "defined".into(),
            Self::Lang(ranges) => format!("lang({})", ranges.join(",")).into(),
            Self::Dir(direction) => format!("dir({direction})").into(),
            Self::Visited => "visited".into(),
            Self::Hover => "hover".into(),
            Self::Active => "active".into(),
            Self::Focus => "focus".into(),
            Self::FocusWithin => "focus-within".into(),
            Self::FocusVisible => "focus-visible".into(),
            Self::Target => "target".into(),
            Self::Indeterminate => "indeterminate".into(),
            Self::Default => "default".into(),
            Self::InRange => "in-range".into(),
            Self::OutOfRange => "out-of-range".into(),
            Self::Autofill => "autofill".into(),
        }
    }
}

impl NonTSPseudoClassTrait for PseudoClass {
    type Impl = Selectors;

    fn is_active_or_hover(&self) -> bool {
        matches!(self, Self::Hover | Self::Active)
    }

    fn is_user_action_state(&self) -> bool {
        matches!(
            self,
            Self::Hover | Self::Active | Self::Focus | Self::FocusWithin | Self::FocusVisible
        )
    }
}

impl ToCss for PseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        write!(dest, ":{}", self.keyword())
    }
}

/// Known pseudo-elements parse like browsers' and match nothing: real
/// `querySelectorAll("p::before")` returns an empty list, never throws
/// (MDN: "If the specified selectors include a CSS pseudo-element, the
/// returned list is always empty"). dom creates no boxes (no layout, no
/// generated content), so every variant refuses to match. Unknown names are
/// still refused at parse time, exactly as browsers refuse them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PseudoElement {
    Before,
    After,
    FirstLine,
    FirstLetter,
    Selection,
    Placeholder,
    Marker,
    Backdrop,
}

impl PseudoElement {
    /// Parses one known pseudo-element name (case-insensitive per CSS).
    fn from_keyword(name: &str) -> Option<Self> {
        const KEYWORDS: &[(&str, PseudoElement)] = &[
            ("before", PseudoElement::Before),
            ("after", PseudoElement::After),
            ("first-line", PseudoElement::FirstLine),
            ("first-letter", PseudoElement::FirstLetter),
            ("selection", PseudoElement::Selection),
            ("placeholder", PseudoElement::Placeholder),
            ("marker", PseudoElement::Marker),
            ("backdrop", PseudoElement::Backdrop),
        ];
        KEYWORDS
            .iter()
            .find(|(keyword, _)| name.eq_ignore_ascii_case(keyword))
            .map(|(_, element)| *element)
    }

    fn keyword(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
            Self::FirstLine => "first-line",
            Self::FirstLetter => "first-letter",
            Self::Selection => "selection",
            Self::Placeholder => "placeholder",
            Self::Marker => "marker",
            Self::Backdrop => "backdrop",
        }
    }
}

impl PseudoElementTrait for PseudoElement {
    type Impl = Selectors;
}

impl ToCss for PseudoElement {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        write!(dest, "::{}", self.keyword())
    }
}

/// Zero-sized witness binding the engine to our name types.
#[derive(Clone, Debug)]
struct Selectors;

impl SelectorImpl for Selectors {
    type ExtraMatchingData<'a> = ();
    type Identifier = Ident;
    type LocalName = TagLocalName;
    type NamespaceUrl = NsUrl;
    type NamespacePrefix = NsPrefix;
    type BorrowedLocalName = str;
    type BorrowedNamespaceUrl = str;
    type AttrValue = AttrValue;
    type NonTSPseudoClass = PseudoClass;
    type PseudoElement = PseudoElement;
}

/// Parse-time policy: structural features on, state and pseudo-elements off.
///
/// No default namespace is declared, so a bare type selector matches its
/// local name in any namespace, standard CSS behavior absent `@namespace`.
#[derive(Debug, Default)]
struct SelectorLanguage;

impl<'i> SelectorParser<'i> for SelectorLanguage {
    type Impl = Selectors;
    type Error = ParseFail;

    fn parse_is_and_where(&self) -> bool {
        true
    }

    fn parse_nth_child_of(&self) -> bool {
        true
    }

    fn parse_has(&self) -> bool {
        true
    }

    /// Known pseudo-elements parse and match nothing; unknown names are
    /// refused exactly as browsers refuse them.
    fn parse_pseudo_element(
        &self,
        location: cssparser::SourceLocation,
        name: cssparser::CowRcStr<'i>,
    ) -> Result<PseudoElement, cssparser::ParseError<'i, Self::Error>> {
        PseudoElement::from_keyword(&name).ok_or_else(|| {
            location.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                name,
            ))
        })
    }

    /// State pseudo-classes with statically knowable truth; anything else is
    /// refused exactly as browsers refuse unknown pseudo-classes.
    fn parse_non_ts_pseudo_class(
        &self,
        location: cssparser::SourceLocation,
        name: cssparser::CowRcStr<'i>,
    ) -> Result<PseudoClass, cssparser::ParseError<'i, Self::Error>> {
        match PseudoClass::from_keyword(&name) {
            Some(class) => Ok(class),
            None => Err(location.new_custom_error(
                SelectorParseErrorKind::UnsupportedPseudoClassOrElement(name),
            )),
        }
    }

    /// Functional state pseudo-classes: `:lang(range)` and `:dir(direction)`
    /// take an argument the engine hands us inside an already-opened block.
    fn parse_non_ts_functional_pseudo_class<'t>(
        &self,
        name: cssparser::CowRcStr<'i>,
        input: &mut CssParser<'i, 't>,
        _after_part: bool,
    ) -> Result<PseudoClass, cssparser::ParseError<'i, Self::Error>> {
        PseudoClass::parse_functional(&name, input)
    }
}

/// The class of a selector-parse failure, stable enough for programmatic
/// handling (`DOMException` mapping at the future js layer) without parsing
/// text back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseFailKind {
    /// Nothing usable was given (`""`, whitespace).
    EmptySelector,
    /// A combinator with nothing on its right-hand side (`div >`).
    DanglingCombinator,
    /// An unknown pseudo-class or pseudo-element name (known
    /// pseudo-elements parse and match nothing, like browsers).
    UnsupportedPseudo,
    /// An identifier appeared where only a selector may (`..x`).
    UnexpectedIdent,
    /// A namespace prefix with no mapping (`svg|circle`: none exist here).
    UnknownNamespacePrefix,
    /// Malformed pieces inside an attribute selector (`[href=]`).
    BadAttributeSelector,
    /// A feature used where the grammar forbids it: compounds inside
    /// compounds, pseudo-elements inside `:is()`.
    MisplacedFeature,
    /// Token-level junk rejected by the CSS lexer itself.
    MalformedInput,
}

/// Why a selector string did not parse: one of the [`ParseFailKind`] classes
/// plus the human-readable rendering of that specific failure.
///
/// `selectors` ships no `Display` for its error kinds; [`SelectError::Syntax`]
/// surfaces this type's text instead. The message stays stable per class;
/// only the offending name varies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseFail {
    kind: ParseFailKind,
    message: Box<str>,
}

impl ParseFail {
    /// The class of failure, independent of wording or position.
    #[must_use]
    pub fn kind(&self) -> ParseFailKind {
        self.kind
    }
}

impl fmt::Display for ParseFail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseFail {}

impl From<SelectorParseErrorKind<'_>> for ParseFail {
    fn from(kind: SelectorParseErrorKind<'_>) -> Self {
        use SelectorParseErrorKind as K;
        let (kind, message): (ParseFailKind, String) = match kind {
            K::NoQualifiedNameInAttributeSelector(_) => (
                ParseFailKind::BadAttributeSelector,
                "attribute selector needs a qualified name".into(),
            ),
            K::EmptySelector => (ParseFailKind::EmptySelector, "empty selector".into()),
            K::DanglingCombinator => (
                ParseFailKind::DanglingCombinator,
                "combinator with nothing on its right".into(),
            ),
            K::NonCompoundSelector => (
                ParseFailKind::MisplacedFeature,
                "only compound selectors may appear inside this compound".into(),
            ),
            K::NonPseudoElementAfterSlotted | K::InvalidPseudoElementAfterSlotted => (
                ParseFailKind::MisplacedFeature,
                "::slotted() must be followed by a pseudo-element".into(),
            ),
            K::InvalidPseudoElementInsideWhere => (
                ParseFailKind::MisplacedFeature,
                "pseudo-elements may not appear inside :is()/:where()".into(),
            ),
            // The engine's internal-state error is a defect indicator, not
            // user grammar misuse; bucketing it as MisplacedFeature would
            // mislead the future DOMException mapper (audit finding L12).
            K::InvalidState => (
                ParseFailKind::MalformedInput,
                "internal selector-parser state error".into(),
            ),
            K::UnexpectedTokenInAttributeSelector(_)
            | K::BadValueInAttr(_)
            | K::InvalidQualNameInAttr(_) => (
                ParseFailKind::BadAttributeSelector,
                "unexpected token in attribute selector".into(),
            ),
            K::PseudoElementExpectedColon(_)
            | K::PseudoElementExpectedIdent(_)
            | K::NoIdentForPseudo(_) => (
                ParseFailKind::MisplacedFeature,
                "malformed pseudo-element".into(),
            ),
            K::UnsupportedPseudoClassOrElement(name) => (
                ParseFailKind::UnsupportedPseudo,
                format!("unsupported pseudo-class or element `{name}`"),
            ),
            K::UnexpectedIdent(name) => (
                ParseFailKind::UnexpectedIdent,
                format!("unexpected identifier `{name}`"),
            ),
            K::ExpectedNamespace(name) => (
                ParseFailKind::UnknownNamespacePrefix,
                format!("unknown namespace prefix `{name}`"),
            ),
            K::ExpectedBarInAttr(_) => (
                ParseFailKind::BadAttributeSelector,
                "expected `|` separating namespace from attribute".into(),
            ),
            K::ExplicitNamespaceUnexpectedToken(_) => (
                ParseFailKind::BadAttributeSelector,
                "unexpected token after namespace separator".into(),
            ),
            K::ClassNeedsIdent(_) => (
                ParseFailKind::MisplacedFeature,
                "class selector needs an identifier".into(),
            ),
        };
        Self {
            kind,
            message: message.into(),
        }
    }
}

// ── Element view ────────────────────────────────────────────────────────────

/// One live element as the engine sees it: a borrowed [`Dom`] plus a handle.
#[derive(Clone, Debug)]
struct DomElement<'a> {
    dom: &'a Dom,
    id: NodeId,
}

impl<'a> DomElement<'a> {
    fn new(dom: &'a Dom, id: NodeId) -> Option<Self> {
        matches!(dom.get(id)?.kind(), NodeKind::Element { .. }).then_some(Self { dom, id })
    }

    /// This element's qualified name, or `None` if it stopped being an
    /// element (impossible mid-query: the borrow freezes mutation).
    fn qual_name(&self) -> Option<&'a markup5ever::QualName> {
        match self.dom.get(self.id)?.kind() {
            NodeKind::Element { name, .. } => Some(name),
            _ => None,
        }
    }

    fn attributes(&self) -> &'a [Attribute] {
        match self.dom.get(self.id) {
            Some(node) => match node.kind() {
                NodeKind::Element { attributes, .. } => attributes,
                _ => &[],
            },
            None => &[],
        }
    }

    /// First no-namespace attribute named `local`, under the element's case
    /// regime. Delegates to [`crate::state::attr_value`]: the one home of
    /// that policy, so `[href]` and `:link` can never drift apart again
    /// (subagent review R3-6).
    fn attr_value(&self, local: &str) -> Option<&'a str> {
        state::attr_value(self.dom, self.id, local)
    }

    /// Whether this element lives in the HTML namespace.
    ///
    /// This is the engine's switch for case handling: when true it asks us
    /// about lowercased tag/attribute names, which is what the tree stores.
    fn is_html_in_html_document(&self) -> bool {
        state::is_html(self.dom, self.id)
    }
}

/// The engine's view of the tree: every question routes through [`Dom`]'s
/// public reads, so matching can never observe a half-mutated arena.
impl Element for DomElement<'_> {
    type Impl = Selectors;

    /// Cache identity for nth-index/`:has` bookkeeping.
    ///
    /// Routed through the arena's slot storage: one live node owns one slot
    /// and the shared borrow freezes addresses, so identity is exact; no
    /// collisions between distinct elements, no aliasing of one element.
    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.dom.cache_identity(self.id))
    }

    fn parent_element(&self) -> Option<Self> {
        let mut cursor = self.dom.parent(self.id)?;
        loop {
            if matches!(self.dom.get(cursor)?.kind(), NodeKind::Element { .. }) {
                return Some(Self {
                    dom: self.dom,
                    id: cursor,
                });
            }
            cursor = self.dom.parent(cursor)?;
        }
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false // no shadow trees in dom v1
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let parent = self.dom.parent(self.id)?;
        let kids = self.dom.children(parent)?;
        let mut last = None;
        for &kid in kids {
            if kid == self.id {
                return last.map(|id| Self { dom: self.dom, id });
            }
            if DomElement::new(self.dom, kid).is_some() {
                last = Some(kid);
            }
        }
        None
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let parent = self.dom.parent(self.id)?;
        let kids = self.dom.children(parent)?;
        let mut passed_self = false;
        for &kid in kids {
            if kid == self.id {
                passed_self = true;
            } else if passed_self && DomElement::new(self.dom, kid).is_some() {
                return Some(Self {
                    dom: self.dom,
                    id: kid,
                });
            }
        }
        None
    }

    fn first_element_child(&self) -> Option<Self> {
        let mut kids = self.dom.children(self.id)?;
        let id = kids
            .find(|&&kid| DomElement::new(self.dom, kid).is_some())
            .copied()?;
        Some(Self { dom: self.dom, id })
    }

    fn is_html_element_in_html_document(&self) -> bool {
        self.is_html_in_html_document()
    }

    /// The engine pre-selects which spelling to ask about (lowercased for
    /// HTML in HTML documents), but hand-built trees are not obliged to hold
    /// lowercased names the way a tokenized one does, so comparison routes
    /// through the shared case-regime policy, insensitive in both
    /// directions when we declared HTML-in-HTML.
    fn has_local_name(&self, local_name: &str) -> bool {
        state::local_is(self.dom, self.id, &[local_name])
    }

    fn has_namespace(&self, ns: &str) -> bool {
        self.qual_name().is_some_and(|name| name.ns.as_ref() == ns)
    }

    /// Same local name and namespace: the relation `nth-of-type` relies on.
    fn is_same_type(&self, other: &Self) -> bool {
        match (self.qual_name(), other.qual_name()) {
            (Some(a), Some(b)) => {
                let (a_name, b_name): (&str, &str) = (&a.local, &b.local);
                a.ns == b.ns
                    && if self.is_html_in_html_document() {
                        a_name.eq_ignore_ascii_case(b_name)
                    } else {
                        a_name == b_name
                    }
            }
            _ => false,
        }
    }

    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&NsUrl>,
        local_name: &TagLocalName,
        operation: &AttrSelectorOperation<&AttrValue>,
    ) -> bool {
        self.attributes().iter().any(|attribute| {
            let namespace_ok = match ns {
                NamespaceConstraint::Any => true,
                NamespaceConstraint::Specific(url) => attribute.name.ns == url.0,
            };
            if !namespace_ok {
                return false;
            }
            // Name casing was already chosen by the engine (see
            // `has_local_name`); value case handling rides inside `operation`.
            let stored = attribute.name.local.as_ref();
            let wanted: &str = &local_name.0;
            let named = if self.is_html_in_html_document() {
                stored.eq_ignore_ascii_case(wanted)
            } else {
                stored == wanted
            };
            named && operation.eval_str(attribute.value.as_str())
        })
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &PseudoClass,
        _context: &mut MatchingContext<Selectors>,
    ) -> bool {
        // Named states are answered by crate::state under its truth
        // policy; this match is only the routing table.
        match pc {
            PseudoClass::AnyLink | PseudoClass::Link => state::is_hyperlink(self.dom, self.id),
            PseudoClass::Enabled => state::is_enabled(self.dom, self.id),
            PseudoClass::Disabled => state::is_disabled(self.dom, self.id),
            PseudoClass::Checked => state::is_checked(self.dom, self.id),
            PseudoClass::Required => state::is_required(self.dom, self.id),
            PseudoClass::Optional => state::is_optional(self.dom, self.id),
            PseudoClass::ReadOnly => state::is_read_only(self.dom, self.id),
            PseudoClass::ReadWrite => state::is_read_write(self.dom, self.id),
            PseudoClass::PlaceholderShown => state::is_placeholder_shown(self.dom, self.id),
            PseudoClass::Defined => state::is_defined(self.dom, self.id),
            PseudoClass::Lang(ranges) => state::lang_matches(self.dom, self.id, ranges),
            PseudoClass::Dir(direction) => state::direction_is(self.dom, self.id, direction),
            // Static subsets of states whose full semantics need runtime
            // context; see crate::state for exactly which clause each
            // covers and which stays deferred.
            PseudoClass::Indeterminate => state::is_indeterminate(self.dom, self.id),
            PseudoClass::Default => state::is_default(self.dom, self.id),
            // Vacuous set: the context these describe (browsing history,
            // URL fragment during a query, numeric range validation,
            // autofill activity) does not exist in a headless tree. A
            // fresh page in a real browser answers the same way: no
            // matches.
            PseudoClass::Visited
            | PseudoClass::Hover
            | PseudoClass::Active
            | PseudoClass::Focus
            | PseudoClass::FocusWithin
            | PseudoClass::FocusVisible
            | PseudoClass::Target
            | PseudoClass::InRange
            | PseudoClass::OutOfRange
            | PseudoClass::Autofill => false,
        }
    }

    fn match_pseudo_element(
        &self,
        pe: &PseudoElement,
        _context: &mut MatchingContext<Selectors>,
    ) -> bool {
        // Known pseudo-elements parse like browsers' and never match: no
        // layout, no boxes, no generated content exists in this tree.
        let _ = pe;
        false
    }

    fn apply_selector_flags(&self, _flags: selectors::matching::ElementSelectorFlags) {
        // No invalidation machinery exists here: queries are one-shot reads.
    }

    /// `:link` / `:any-link`: a hyperlink in any namespace carrying an
    /// `href` (see `crate::state::is_hyperlink` for the spec citations).
    fn is_link(&self) -> bool {
        state::is_hyperlink(self.dom, self.id)
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &Ident, case_sensitivity: selectors::attr::CaseSensitivity) -> bool {
        self.attr_value("id")
            .is_some_and(|value| case_sensitivity.eq(value.as_bytes(), id.0.as_bytes()))
    }

    fn has_class(&self, name: &Ident, case_sensitivity: selectors::attr::CaseSensitivity) -> bool {
        self.attr_value("class").is_some_and(|value| {
            value
                .split_ascii_whitespace()
                .any(|token| case_sensitivity.eq(token.as_bytes(), name.0.as_bytes()))
        })
    }

    fn has_custom_state(&self, _name: &Ident) -> bool {
        false
    }

    fn imported_part(&self, _name: &Ident) -> Option<Ident> {
        None
    }

    fn is_part(&self, _name: &Ident) -> bool {
        false
    }

    /// `:empty` ignores comments and doctypes; empty text counts as nothing.
    fn is_empty(&self) -> bool {
        let Some(mut kids) = self.dom.children(self.id) else {
            return true; // unreachable mid-query: mutation is frozen by the borrow
        };
        kids.all(|&kid| match self.dom.get(kid).map(|node| node.kind()) {
            Some(NodeKind::Text { data }) => data.is_empty(),
            Some(NodeKind::Element { .. }) => false,
            _ => true,
        })
    }

    /// The document root's only child: the tree's root element.
    fn is_root(&self) -> bool {
        matches!(
            self.dom.parent(self.id),
            Some(parent) if matches!(
                self.dom.get(parent).map(|node| node.kind()),
                Some(NodeKind::Document)
            )
        )
    }

    fn add_element_unique_hashes(&self, _filter: &mut selectors::bloom::BloomFilter) -> bool {
        false // no bloom filter is supplied to queries
    }
}

// ── Search ──────────────────────────────────────────────────────────────────

/// Every descendant of `scope` in document order, scope itself excluded:
/// the candidate set of a scoped query. Iterative (an explicit stack of
/// child-list cursors), so tree depth costs nothing but bookkeeping.
struct Descendants<'a> {
    dom: &'a Dom,
    stack: Vec<std::slice::Iter<'a, NodeId>>,
}

impl<'a> Descendants<'a> {
    fn new(dom: &'a Dom, scope: NodeId) -> Self {
        Self {
            dom,
            stack: dom
                .children(scope)
                .map(|kids| vec![kids])
                .unwrap_or_default(),
        }
    }
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        while let Some(top) = self.stack.last_mut() {
            match top.next() {
                Some(&id) => {
                    if let Some(kids) = self.dom.children(id) {
                        self.stack.push(kids);
                    }
                    return Some(id);
                }
                None => {
                    self.stack.pop();
                }
            }
        }
        None
    }
}

impl Dom {
    /// Compiles a selector list once per query.
    ///
    /// Callers that run the same selector in a hot loop should keep asking
    /// through this string API for now; a compiled-list entry point is
    /// deliberately deferred until the `js` layer proves it needs one.
    fn compile(selectors: &str) -> Result<SelectorList<Selectors>, SelectError> {
        let mut input = ParserInput::new(selectors);
        let mut parser = CssParser::new(&mut input);
        SelectorList::parse(&SelectorLanguage, &mut parser, ParseRelative::No).map_err(|error| {
            // Engine-classified failures carry their kind; token-level
            // junk keeps the CSS lexer's own wording under
            // [`ParseFailKind::MalformedInput`].
            let fail = match error.kind {
                cssparser::ParseErrorKind::Custom(fail) => fail,
                cssparser::ParseErrorKind::Basic(basic) => ParseFail {
                    kind: ParseFailKind::MalformedInput,
                    message: basic.to_string().into(),
                },
            };
            SelectError::Syntax(fail)
        })
    }

    /// Shared scan behind [`Dom::select_all`] and [`Dom::select_first`]:
    /// walks candidates in document order, stopping after `limit` hits.
    fn find_matches(
        &self,
        list: &SelectorList<Selectors>,
        scope: NodeId,
        limit: Option<usize>,
    ) -> Vec<NodeId> {
        let mut caches = SelectorCaches::default();
        // One context (and its caches) serves the whole scan; matching is a
        // read, so nothing here can invalidate the arena underneath it.
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            self.quirks_mode().engine(),
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        let mut hits = Vec::new();
        for candidate in Descendants::new(self, scope) {
            let Some(element) = DomElement::new(self, candidate) else {
                continue; // text, comments, doctype: never matchable
            };
            if matches_selector_list(list, &element, &mut context) {
                hits.push(candidate);
                if limit.is_some_and(|max| hits.len() >= max) {
                    break;
                }
            }
        }
        hits
    }

    /// Every descendant of `scope` matching the selector list, in document
    /// order: `querySelectorAll` semantics. The scope node itself is not a
    /// candidate; ancestors above it remain visible to combinators, exactly
    /// as in browsers.
    ///
    /// # Errors
    ///
    /// - [`SelectError::StaleNode`] if `scope` names a destroyed node.
    /// - [`SelectError::Syntax`] if `selectors` does not parse.
    pub fn select_all(&self, scope: NodeId, selectors: &str) -> Result<Vec<NodeId>, SelectError> {
        let list = Self::compile(selectors)?;
        if !self.contains(scope) {
            return Err(SelectError::StaleNode);
        }
        Ok(self.find_matches(&list, scope, None))
    }

    /// The first matching descendant of `scope` in document order:
    /// `querySelector` semantics. Stops scanning at the first hit.
    ///
    /// # Errors
    ///
    /// Same as [`Dom::select_all`].
    pub fn select_first(
        &self,
        scope: NodeId,
        selectors: &str,
    ) -> Result<Option<NodeId>, SelectError> {
        let list = Self::compile(selectors)?;
        if !self.contains(scope) {
            return Err(SelectError::StaleNode);
        }
        Ok(self.find_matches(&list, scope, Some(1)).into_iter().next())
    }

    /// Whether one element matches the selector list: `Element.matches`
    /// semantics. Matching may walk this element's real ancestors and
    /// siblings, wherever it sits.
    ///
    /// # Errors
    ///
    /// - [`SelectError::StaleNode`] if `element` names a destroyed node.
    /// - [`SelectError::NotAnElement`] if `element` names a non-element node.
    /// - [`SelectError::Syntax`] if `selectors` does not parse.
    pub fn matches(&self, element: NodeId, selectors: &str) -> Result<bool, SelectError> {
        let list = Self::compile(selectors)?;
        let Some(view) = DomElement::new(self, element) else {
            return Err(if self.contains(element) {
                SelectError::NotAnElement
            } else {
                SelectError::StaleNode
            });
        };
        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            self.quirks_mode().engine(),
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        Ok(matches_selector_list(&list, &view, &mut context))
    }
}
