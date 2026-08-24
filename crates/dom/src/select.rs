//! Selector matching over the arena, powered by Servo's `selectors` engine.
//!
//! Search takes a CSS selector string, compiles it against our name types,
//! and walks descendants in document order — `querySelector` /
//! `querySelectorAll` semantics, minus pseudo-elements (they never create
//! nodes, so a query naming one is a syntax error, as in browsers).
//!
//! State pseudo-classes parse like browsers' and evaluate against what a
//! static headless tree can truthfully know: `:link` matches HTML link
//! elements carrying an `href`; `:hover`, `:active`, `:focus`, and
//! `:visited` match nothing (no input devices, no browsing history); any
//! other pseudo-class is refused, also as in browsers.
//!
//! Selector *names* are newtypes over the same interned atoms elements carry
//! ([`crate::QualName`]), so a compiled selector compares names without ever
//! materializing strings.

use std::borrow::Borrow;
use std::fmt;

use cssparser::{Parser as CssParser, ParserInput, ToCss};
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

use crate::arena::Dom;
use crate::id::NodeId;
use crate::node::{Attribute, NodeKind};

/// The document-compatibility mode a query runs under — what html5ever's
/// tree builder reports and parsed pages carry.
///
/// It changes exactly one matching behavior here: in full quirks mode,
/// class and id selector values compare ASCII-case-insensitively (the
/// WHATWG id/class quirk). Standards and limited-quirks modes stay exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuirksMode {
    /// Standards mode: full CSS case rules.
    NoQuirks,
    /// Limited quirks: same selector rules as standards mode.
    LimitedQuirks,
    /// Full quirks: legacy case-insensitive class/id matching.
    Quirks,
}

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
    /// A namespace prefix (`svg|circle`) — never resolved; see [`SelectorLanguage`].
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
    // DJB2 (Daniel J. Bernstein): seed 5381, wrap-multiply by 33, add byte —
    // the same constant recipe markup5ever's atoms use, so both sides of a
    // comparison hash alike.
    fn precomputed_hash(&self) -> u32 {
        self.0.bytes().fold(5381_u32, |hash, byte| {
            hash.wrapping_mul(33).wrapping_add(u32::from(byte))
        })
    }
}

/// State pseudo-classes: parsed like browsers, matched against what a
/// static headless tree can truthfully know.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PseudoClass {
    /// `:link` — an HTML link element (`a`/`area`/`link`) with an `href`.
    Link,
    /// `:visited` — never matches; browsing history does not exist here.
    Visited,
    /// `:hover` — never matches; there is no pointer.
    Hover,
    /// `:active` — never matches; nothing is being clicked.
    Active,
    /// `:focus` — never matches until a focus owner can exist (js layer).
    Focus,
}

impl PseudoClass {
    /// Parses one pseudo-class keyword (CSS keywords are case-insensitive).
    fn from_keyword(name: &str) -> Option<Self> {
        let candidate = |expected: &str| name.eq_ignore_ascii_case(expected);
        if candidate("link") {
            Some(Self::Link)
        } else if candidate("visited") {
            Some(Self::Visited)
        } else if candidate("hover") {
            Some(Self::Hover)
        } else if candidate("active") {
            Some(Self::Active)
        } else if candidate("focus") {
            Some(Self::Focus)
        } else {
            None
        }
    }

    /// The canonical lowercase keyword, for CSS serialization.
    fn keyword(self) -> &'static str {
        match self {
            Self::Link => "link",
            Self::Visited => "visited",
            Self::Hover => "hover",
            Self::Active => "active",
            Self::Focus => "focus",
        }
    }
}

impl NonTSPseudoClassTrait for PseudoClass {
    type Impl = Selectors;

    fn is_active_or_hover(&self) -> bool {
        matches!(self, Self::Hover | Self::Active)
    }

    fn is_user_action_state(&self) -> bool {
        matches!(self, Self::Hover | Self::Active | Self::Focus)
    }
}

impl ToCss for PseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        write!(dest, ":{}", self.keyword())
    }
}

/// Never constructed: pseudo-elements (`::before`) generate no nodes here,
/// so querying for them is refused at parse time, like browsers do.
#[derive(Clone, Debug, Eq, PartialEq)]
enum PseudoElement {}

impl PseudoElementTrait for PseudoElement {
    type Impl = Selectors;
}

impl ToCss for PseudoElement {
    fn to_css<W: fmt::Write>(&self, _dest: &mut W) -> fmt::Result {
        match *self {}
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
/// local name in any namespace — standard CSS behavior absent `@namespace`.
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
}

/// The class of a selector-parse failure — stable enough for programmatic
/// handling (`DOMException` mapping at the future js layer) without parsing
/// text back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseFailKind {
    /// Nothing usable was given (`""`, whitespace).
    EmptySelector,
    /// A combinator with nothing on its right-hand side (`div >`).
    DanglingCombinator,
    /// An unknown pseudo-class, or any pseudo-element (dom creates no nodes
    /// for those).
    UnsupportedPseudo,
    /// An identifier appeared where only a selector may (`..x`).
    UnexpectedIdent,
    /// A namespace prefix with no mapping (`svg|circle` — none exist here).
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
/// surfaces this type's text instead. The message stays stable per class —
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
            K::InvalidState => (
                ParseFailKind::MisplacedFeature,
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

    /// First attribute whose name equals `local` under the element's case
    /// regime: exact for non-HTML, ASCII-insensitive for HTML-in-HTML, so
    /// hand-built mixed-case attributes behave like tokenized ones.
    ///
    /// Only no-namespace attributes are considered; that is where the
    /// engine's `class`, `id`, and unqualified `[attr]` selectors live.
    fn attr_value(&self, local: &str) -> Option<&'a str> {
        let html = self.is_html_in_html_document();
        self.attributes()
            .iter()
            .find(|attribute| {
                let stored = attribute.name.local.as_ref();
                let named = if html {
                    stored.eq_ignore_ascii_case(local)
                } else {
                    stored == local
                };
                attribute.name.ns.is_empty() && named
            })
            .map(|attribute| attribute.value.as_str())
    }

    /// Whether this element's namespace is HTML inside an HTML document.
    ///
    /// This is the engine's switch for case handling: when true it asks us
    /// about lowercased tag/attribute names, which is what the tree stores.
    fn is_html_in_html_document(&self) -> bool {
        self.qual_name()
            .is_some_and(|name| name.ns == crate::node::html_namespace())
    }
}

/// The engine's view of the tree: every question routes through [`Dom`]'s
/// public reads, so matching can never observe a half-mutated arena.
impl Element for DomElement<'_> {
    type Impl = Selectors;

    /// Cache identity for nth-index/`:has` bookkeeping.
    ///
    /// Routed through the arena's slot storage: one live node owns one slot
    /// and the shared borrow freezes addresses, so identity is exact — no
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
    /// lowercased names the way a tokenized one does — so when we declared
    /// HTML-in-HTML, comparison is case-insensitive in both directions.
    fn has_local_name(&self, local_name: &str) -> bool {
        self.qual_name().is_some_and(|name| {
            let stored = name.local.as_ref();
            if self.is_html_in_html_document() {
                stored.eq_ignore_ascii_case(local_name)
            } else {
                stored == local_name
            }
        })
    }

    fn has_namespace(&self, ns: &str) -> bool {
        self.qual_name().is_some_and(|name| name.ns.as_ref() == ns)
    }

    /// Same local name and namespace — the relation `nth-of-type` relies on.
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
        match pc {
            PseudoClass::Link => self.is_link(),
            // No pointer, no keyboard focus owner, no browsing history: the
            // user-action states are truthful vacuous misses — exactly what
            // a fresh static page answers in a real browser.
            PseudoClass::Hover | PseudoClass::Active | PseudoClass::Focus
            | PseudoClass::Visited => false,
        }
    }

    fn match_pseudo_element(
        &self,
        pe: &PseudoElement,
        _context: &mut MatchingContext<Selectors>,
    ) -> bool {
        match *pe {}
    }

    fn apply_selector_flags(&self, _flags: selectors::matching::ElementSelectorFlags) {
        // No invalidation machinery exists here: queries are one-shot reads.
    }

    /// `:link` — an HTML link element carrying an `href`.
    ///
    /// Local names compare under this element's case regime (ASCII-
    /// insensitive for HTML-in-HTML), like every other name check here;
    /// hand-built `<A HREF="…">` counts exactly like tokenized output.
    fn is_link(&self) -> bool {
        self.is_html_in_html_document()
            && ["a", "area", "link"].iter().any(|name| self.has_local_name(name))
            && self.attr_value("href").is_some()
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

/// Every descendant of `scope` in document order, scope itself excluded —
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
        SelectorList::parse(&SelectorLanguage, &mut parser, ParseRelative::No)
            .map_err(|error| {
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
        quirks_mode: QuirksMode,
    ) -> Vec<NodeId> {
        let mut caches = SelectorCaches::default();
        // One context (and its caches) serves the whole scan; matching is a
        // read, so nothing here can invalidate the arena underneath it.
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            quirks_mode.engine(),
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
    /// order — `querySelectorAll` semantics. The scope node itself is not a
    /// candidate; ancestors above it remain visible to combinators, exactly
    /// as in browsers.
    ///
    /// # Errors
    ///
    /// - [`SelectError::StaleNode`] if `scope` names a destroyed node.
    /// - [`SelectError::Syntax`] if `selectors` does not parse.
    pub fn select_all(
        &self,
        scope: NodeId,
        selectors: &str,
        quirks_mode: QuirksMode,
    ) -> Result<Vec<NodeId>, SelectError> {
        let list = Self::compile(selectors)?;
        if !self.contains(scope) {
            return Err(SelectError::StaleNode);
        }
        Ok(self.find_matches(&list, scope, None, quirks_mode))
    }

    /// The first matching descendant of `scope` in document order —
    /// `querySelector` semantics. Stops scanning at the first hit.
    ///
    /// # Errors
    ///
    /// Same as [`Dom::select_all`].
    pub fn select_first(
        &self,
        scope: NodeId,
        selectors: &str,
        quirks_mode: QuirksMode,
    ) -> Result<Option<NodeId>, SelectError> {
        let list = Self::compile(selectors)?;
        if !self.contains(scope) {
            return Err(SelectError::StaleNode);
        }
        Ok(self
            .find_matches(&list, scope, Some(1), quirks_mode)
            .into_iter()
            .next())
    }

    /// Whether one element matches the selector list — `Element.matches`
    /// semantics. Matching may walk this element's real ancestors and
    /// siblings, wherever it sits.
    ///
    /// # Errors
    ///
    /// - [`SelectError::StaleNode`] if `element` names a destroyed node.
    /// - [`SelectError::NotAnElement`] if `element` names a non-element node.
    /// - [`SelectError::Syntax`] if `selectors` does not parse.
    pub fn matches(
        &self,
        element: NodeId,
        selectors: &str,
        quirks_mode: QuirksMode,
    ) -> Result<bool, SelectError> {
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
            quirks_mode.engine(),
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        Ok(matches_selector_list(&list, &view, &mut context))
    }
}
