//! `DOMImplementation`, DOM parsing, and serialization.

use super::{
    NodeContext,
    clone::{import_snapshot, materialize_import},
    create_kind, throw_dom, throw_dom_error, validate_and_extract, world, world_for_node,
    wrap_new_document, wrap_new_document_in_world,
};

use dom::{LocalName, NodeId, NodeKind, QualName, html_namespace};

use rquickjs::{Ctx, Exception, Result, Value, class::Trace};

use crate::js::world::Handle;

/// `DOMImplementation` as a platform object
/// (<https://dom.spec.whatwg.org/#interface-domimplementation>).
#[derive(Trace, rquickjs::JsLifetime)]
pub struct JsImplementation {
    pub(crate) document: Handle,
}

include!(concat!(env!("OUT_DIR"), "/DOMImplementation.rs"));

#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "generated dispatch passes Ctx by value and invokes operations on the receiver"
)]
impl JsImplementation {
    // https://dom.spec.whatwg.org/#dom-domimplementation-hasfeature
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated operations share one fallible call shape"
    )]
    fn has_feature(&self, _ctx: Ctx<'_>) -> Result<bool> {
        Ok(true)
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createdocumenttype
    fn create_document_type<'js>(
        &self,
        ctx: Ctx<'js>,
        name: rquickjs::String<'js>,
        public_id: rquickjs::String<'js>,
        system_id: rquickjs::String<'js>,
    ) -> Result<Value<'js>> {
        let name = name.to_string()?;
        let public_id = public_id.to_string()?;
        let system_id = system_id.to_string()?;
        if !valid_doctype_name(&name) {
            return Err(throw_dom(
                &ctx,
                "InvalidCharacterError",
                "doctype name contains invalid characters",
            ));
        }
        create_kind(&ctx, self.document.0, |dom| {
            dom.create_doctype(name, public_id, system_id)
        })
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createdocument
    fn create_document<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: Option<rquickjs::String<'js>>,
        qualified: rquickjs::String<'js>,
        doctype: Option<NodeId>,
    ) -> Result<Value<'js>> {
        let namespace = namespace
            .map(|value| value.to_string())
            .transpose()?
            .unwrap_or_default();
        let qualified = qualified.to_string()?;
        let content_type = match namespace.as_str() {
            "http://www.w3.org/1999/xhtml" => "application/xhtml+xml",
            "http://www.w3.org/2000/svg" => "image/svg+xml",
            _ => "application/xml",
        };
        // `qualifiedName` validates before the doctype steps run
        // (<https://dom.spec.whatwg.org/#dom-domimplementation-createdocument>).
        let root = if qualified.is_empty() {
            None
        } else {
            Some(validate_and_extract(
                &ctx,
                (!namespace.is_empty()).then_some(namespace.as_str()),
                &qualified,
                NodeContext::Element,
            )?)
        };
        let mut parsed = crate::Parsed::empty(content_type);
        let document = parsed.document.document();
        if let Some(doctype) = doctype {
            let owner_rc = world_for_node(&ctx, doctype)?;
            let snapshot = {
                let owner = owner_rc.borrow();
                let Some(source) = owner.document(doctype) else {
                    return Err(Exception::throw_type(&ctx, "no document"));
                };
                import_snapshot(&source.document, doctype, true)
            };
            if let Some(snapshot) = snapshot {
                let node = materialize_import(&mut parsed.document, &snapshot)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
                dom::mutation::append(&mut parsed.document, document, node)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
            }
        }
        if let Some(name) = root {
            let element = parsed.document.create_element(name, Vec::new());
            dom::mutation::append(&mut parsed.document, document, element)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        wrap_new_document_in_world(&ctx, parsed, &world_for_node(&ctx, self.document.0)?)
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createhtmldocument
    fn create_html_document<'js>(
        &self,
        ctx: Ctx<'js>,
        title: Option<rquickjs::String<'js>>,
    ) -> Result<Value<'js>> {
        let mut parsed = crate::Parsed::empty("text/html");
        let document = parsed.document.document();
        let doctype = parsed.document.create_doctype("html", "", "");
        dom::mutation::append(&mut parsed.document, document, doctype)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        let html = parsed
            .document
            .create_element(html_element_name("html"), Vec::new());
        dom::mutation::append(&mut parsed.document, document, html)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        let head = parsed
            .document
            .create_element(html_element_name("head"), Vec::new());
        dom::mutation::append(&mut parsed.document, html, head)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        if let Some(title) = title {
            let title_element = parsed
                .document
                .create_element(html_element_name("title"), Vec::new());
            dom::mutation::append(&mut parsed.document, head, title_element)
                .map_err(|err| throw_dom_error(&ctx, err))?;
            let text = parsed
                .document
                .create_text(dom::DomString::from_utf16(title.to_utf16()?));
            dom::mutation::append(&mut parsed.document, title_element, text)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let body = parsed
            .document
            .create_element(html_element_name("body"), Vec::new());
        dom::mutation::append(&mut parsed.document, html, body)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        wrap_new_document_in_world(&ctx, parsed, &world_for_node(&ctx, self.document.0)?)
    }
}

/// The doctype's name, public id, and system id, when `parsed` holds `id`.
pub(super) fn doctype_fields(
    parsed: &crate::Parsed,
    id: NodeId,
) -> Option<(String, String, String)> {
    match parsed.document.kind(id) {
        Some(NodeKind::Doctype {
            name,
            public_id,
            system_id,
        }) => Some((name.clone(), public_id.clone(), system_id.clone())),
        _ => None,
    }
}

/// An HTML-namespace qualified name for document construction.
fn html_element_name(local: &str) -> QualName {
    QualName::new(None, html_namespace(), LocalName::from(local))
}

/// [Valid doctype name](https://dom.spec.whatwg.org/#valid-doctype-name): no
/// ASCII whitespace, NULL, or `>`.
fn valid_doctype_name(name: &str) -> bool {
    !name
        .chars()
        .any(|c| matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ' | '\0' | '>'))
}

/// `DOMParser` (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-parsing-and-serialization>).
///
/// The JS surface lives in Web IDL; `parse_from_string` is the platform
/// algorithm the generated dispatcher calls.
#[derive(Trace, rquickjs::JsLifetime)]
pub struct JsDomParser {
    /// The constructing realm's document URL. `parseFromString` parses with
    /// the context object's environment settings, not the caller's, so the
    /// instance remembers it; a directly-reached native cannot forge another
    /// document's URL either
    /// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
    url: String,
}

include!(concat!(env!("OUT_DIR"), "/DOMParser.rs"));

#[allow(
    clippy::needless_pass_by_value,
    reason = "generated dispatch passes Ctx and Value by value"
)]
impl JsDomParser {
    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-domparser
    fn new(ctx: &Ctx<'_>) -> Result<Self> {
        Ok(Self {
            url: world(ctx)?.borrow().document_url.as_str().to_owned(),
        })
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring
    fn parse_from_string<'js>(
        &self,
        ctx: Ctx<'js>,
        source: rquickjs::String<'js>,
        type_: dom_parser_generated::DOMParserSupportedType,
    ) -> Result<Value<'js>> {
        let source = source.to_string()?;
        let content_type = type_.as_str();
        // `DOMParser` parses with scripting disabled
        // (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
        let mut parsed = if content_type == "text/html" {
            let mut parsed = crate::parse_html_without_scripting(&source);
            parsed.content_type = content_type;
            parsed.ready_state = crate::ReadyState::Complete;
            parsed
        } else {
            crate::xml::parse_document(&source, content_type)
        };
        // The instance's constructing realm decides the URL, never the
        // caller and never a forged argument: cross-realm method calls parse
        // with the parser's environment, and the reachable native takes no
        // URL from script at all.
        parsed.url = Some(self.url.clone());
        wrap_new_document(&ctx, parsed)
    }
}

/// Wraps the native `DOMParser` so every instance remembers the URL of the
/// realm that constructed it; the parsed document takes that URL
/// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
/// Deflated by `build.rs`, inflated once per process.
pub(crate) const INSTALL_DOMPARSER_CTOR_DEFLATE: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/js_blobs/dom_parser_ctor.deflate"
));

/// The `DOMParser` constructor shim, inflated once per process.
pub(crate) fn install_domparser_ctor_js(ctx: &Ctx<'_>) -> Result<&'static str> {
    static CACHE: std::sync::OnceLock<Box<str>> = std::sync::OnceLock::new();
    crate::js::blob::decompress(ctx, INSTALL_DOMPARSER_CTOR_DEFLATE, &CACHE)
}

/// `XMLSerializer` (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#xmlserializer>).
///
/// The JS surface lives in Web IDL; `serialize_to_string` is the platform
/// algorithm the generated dispatcher calls.
#[derive(Trace, rquickjs::JsLifetime)]
pub struct JsXmlSerializer {
    pub(crate) _reserved: Option<Handle>,
}

include!(concat!(env!("OUT_DIR"), "/XMLSerializer.rs"));

#[allow(
    clippy::needless_pass_by_value,
    reason = "generated dispatch passes Ctx and Value by value"
)]
impl JsXmlSerializer {
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated constructors share one fallible call shape; this payload cannot fail"
    )]
    fn new(_ctx: &Ctx<'_>) -> Result<Self> {
        Ok(Self { _reserved: None })
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-xmlserializer-serializetostring
    #[allow(
        clippy::unused_self,
        reason = "generated dispatch calls every operation on the receiver"
    )]
    fn serialize_to_string<'js>(
        &self,
        ctx: Ctx<'js>,
        root: super::host::NodeReference,
    ) -> Result<rquickjs::String<'js>> {
        // An `Attr` serializes as the empty string
        // (<https://w3c.github.io/DOM-Parsing/#dfn-xml-serialization-algorithm>).
        let super::host::NodeReference::Tree(id) = root else {
            return rquickjs::String::from_str(ctx.clone(), "");
        };
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(id) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let markup = crate::serialize::serialize_xml(&parsed.document, id, false)
            .map_err(|err| throw_dom(&ctx, "InvalidStateError", &err.to_string()))?;
        super::dom_string(&ctx, &markup)
    }
}
