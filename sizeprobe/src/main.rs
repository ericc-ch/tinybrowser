fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();

    #[cfg(feature = "html")]
    if mode == "html" {
        use html5gum::{Token, Tokenizer};
        let src = r##"<html><body><p class="x">hi</p><a href="#">l</a></body></html>"##;
        let mut start_tags = 0usize;
        for tok in Tokenizer::new(src) {
            if matches!(tok, Ok(Token::StartTag(_))) {
                start_tags += 1;
            }
        }
        println!("html5gum: {start_tags} start tags");
    }

    #[cfg(feature = "net")]
    if mode == "net" {
        match ureq::get("http://127.0.0.1:9/probe")
            .header("user-agent", "sizeprobe")
            .call()
        {
            Ok(_) => println!("ureq: unexpected success"),
            Err(e) => println!("ureq: linked, dial failed as expected ({e})"),
        }
    }

    #[cfg(feature = "js")]
    if mode == "js" {
        let rt = rquickjs::Runtime::new().unwrap();
        rt.set_memory_limit(64 * 1024 * 1024);
        rt.set_max_stack_size(1024 * 1024);
        let ctx = rquickjs::Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            let v: i32 = ctx.eval("1 + 1").unwrap();
            let s: String = ctx.eval("'wasm' + '-free'").unwrap();
            println!("quickjs-ng: eval={v} str={s}");
        });
    }

    #[cfg(feature = "h5e")]
    if mode == "h5e" {
        let n = h5e_probe::run();
        println!("html5ever+rcdom: {n} elements");
    }

    #[cfg(feature = "servo")]
    if mode == "servo" {
        use selectors::parser::{ParseRelative, SelectorList};
        use cssparser::{Parser as CssParser, ParserInput};
        let nodes = h5e_probe::run();
        let mut input = ParserInput::new("div.x > p#y:hover, a[href]");
        let mut parser = CssParser::new(&mut input);
        let list =
            SelectorList::parse(&servo_probe::P, &mut parser, ParseRelative::No).expect("selector parse");
        println!(
            "html5ever+selectors: {nodes} elements, {} selectors parsed",
            list.slice().len()
        );
    }

    println!("done [{mode}]");
}

#[cfg(feature = "h5e")]
mod h5e_probe {
    use std::io::Cursor;
    use html5ever::parse_document;
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::{Handle, NodeData, RcDom};

    pub fn run() -> usize {
        let html = r##"<html><body><div class="x"><p id="y">hi</p></div><a href="#">l</a></body></html>"##;
        let dom: RcDom = parse_document(RcDom::default(), Default::default())
            .from_utf8()
            .read_from(&mut Cursor::new(html.as_bytes()))
            .unwrap();
        let mut n = 0usize;
        count_elements(&dom.document, &mut n);
        n
    }

    fn count_elements(h: &Handle, n: &mut usize) {
        if matches!(h.data, NodeData::Element { .. }) {
            *n += 1;
        }
        for c in h.children.borrow().iter() {
            count_elements(c, n);
        }
    }
}

#[cfg(feature = "servo")]
mod servo_probe {
    use cssparser::{CowRcStr, ParseError, ToCss};
    use precomputed_hash::PrecomputedHash;
    use selectors::parser::{self, SelectorParseErrorKind};

    #[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
    pub struct Str(pub String);

    impl<'a> From<&'a str> for Str {
        fn from(s: &'a str) -> Self {
            Str(s.into())
        }
    }

    impl PrecomputedHash for Str {
        fn precomputed_hash(&self) -> u32 {
            self.0.bytes().fold(5381u32, |h, b| {
                h.wrapping_mul(33).wrapping_add(u32::from(b))
            })
        }
    }

    impl ToCss for Str {
        fn to_css<W: std::fmt::Write>(&self, d: &mut W) -> std::fmt::Result {
            d.write_str(&self.0)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct S;

    impl parser::SelectorImpl for S {
        type ExtraMatchingData<'a> = ();
        type AttrValue = Str;
        type Identifier = Str;
        type LocalName = Str;
        type NamespaceUrl = Str;
        type NamespacePrefix = Str;
        type BorrowedLocalName = Str;
        type BorrowedNamespaceUrl = Str;
        type NonTSPseudoClass = Pseudo;
        type PseudoElement = PseudoEl;
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Pseudo {
        Hover,
    }

    impl parser::NonTSPseudoClass for Pseudo {
        type Impl = S;
        fn is_active_or_hover(&self) -> bool {
            matches!(self, Pseudo::Hover)
        }
        fn is_user_action_state(&self) -> bool {
            false
        }
        fn visit<V>(&self, _v: &mut V) -> bool
        where
            V: selectors::visitor::SelectorVisitor<Impl = Self::Impl>,
        {
            true
        }
    }

    impl ToCss for Pseudo {
        fn to_css<W: std::fmt::Write>(&self, d: &mut W) -> std::fmt::Result {
            d.write_str(":hover")
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PseudoEl {}

    impl parser::PseudoElement for PseudoEl {
        type Impl = S;
    }

    impl ToCss for PseudoEl {
        fn to_css<W: std::fmt::Write>(&self, _d: &mut W) -> std::fmt::Result {
            match *self {}
        }
    }

    pub struct P;

    impl<'i> parser::Parser<'i> for P {
        type Impl = S;
        type Error = SelectorParseErrorKind<'i>;

        fn parse_non_ts_pseudo_class(
            &self,
            loc: cssparser::SourceLocation,
            name: CowRcStr<'i>,
        ) -> Result<Pseudo, ParseError<'i, Self::Error>> {
            match &*name {
                "hover" => Ok(Pseudo::Hover),
                _ => Err(ParseError {
                    kind: cssparser::ParseErrorKind::Custom(
                        SelectorParseErrorKind::UnsupportedPseudoClassOrElement(name),
                    ),
                    location: loc,
                }),
            }
        }
    }
}