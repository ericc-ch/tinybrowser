//! `DOMImplementation`, DOM parsing, and serialization.

use super::{
    NodeContext, create_node, detach_for_adopt, import_snapshot, materialize_import,
    retarget_wrapper, throw_dom, throw_dom_error, validate_and_extract, world, world_for_node,
    wrap_new_document, wrap_new_document_in_world, wrap_node,
};

use crate::js::world::Handle;
use crate::js::world::NodeId;

use markup5ever::{LocalName, QualName};

use rquickjs::{Ctx, Result, Value, class::Trace};

/// `DOMImplementation` as a platform object
/// (<https://dom.spec.whatwg.org/#interface-domimplementation>).
#[derive(Trace, rquickjs::JsLifetime)]
pub struct JsImplementation {
    pub(crate) document: Handle,
}

include!(concat!(env!("OUT_DIR"), "/DOMImplementation.rs"));

impl<'js> dom_implementation_generated::DOMImplementation<'js> for JsImplementation {
    // https://dom.spec.whatwg.org/#dom-domimplementation-hasfeature
    fn has_feature(&self, _ctx: Ctx<'js>) -> Result<bool> {
        Ok(true)
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createdocumenttype
    fn create_document_type(
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
        // https://dom.spec.whatwg.org/#dom-domimplementation-createdocumenttype
        create_node(&ctx, self.document.0, |parsed| {
            parsed.document.create_doctype(name, public_id, system_id)
        })
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createdocument
    fn create_document(
        &self,
        ctx: Ctx<'js>,
        namespace: Option<rquickjs::String<'js>>,
        qualified: rquickjs::String<'js>,
        doctype: Option<super::host::NodeReference>,
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
        let world_rc = world_for_node(&ctx, self.document.0)?;
        let font_ctx = world_rc.borrow().runtime.font_ctx.clone();
        let parsed = crate::Parsed::script(content_type, font_ctx);
        // The element is created after the doctype is appended
        // (<https://dom.spec.whatwg.org/#dom-domimplementation-createdocument>).
        let document_root = world_rc.borrow_mut().add_document(parsed);
        world_rc
            .borrow()
            .registry()
            .borrow_mut()
            .insert_document(document_root.document_id(), &world_rc);
        if let Some(super::host::NodeReference::Tree(doctype_id)) = doctype {
            append_adopted_doctype(&ctx, document_root, doctype_id)?;
        }
        if let Some(name) = root {
            let world = world_rc.borrow();
            let Some(mut parsed) = world.document_mut(document_root) else {
                return Err(rquickjs::Exception::throw_type(&ctx, "no document"));
            };
            let element = parsed
                .document
                .base
                .mutate()
                .create_element(name, Vec::new());
            parsed
                .document
                .base
                .mutate()
                .append_children(document_root.node, &[element]);
        }
        wrap_node(&ctx, document_root)
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createhtmldocument
    fn create_html_document(
        &self,
        ctx: Ctx<'js>,
        title: Option<rquickjs::String<'js>>,
    ) -> Result<Value<'js>> {
        let font_ctx = world(&ctx)?.borrow().runtime.font_ctx.clone();
        let mut parsed = crate::Parsed::script("text/html", font_ctx);
        // https://dom.spec.whatwg.org/#dom-domimplementation-createhtmldocument
        let document_root = parsed.document.base.root_node().id;
        let doctype =
            parsed
                .document
                .create_doctype("html".to_owned(), String::new(), String::new());
        parsed
            .document
            .base
            .mutate()
            .append_children(document_root, &[doctype]);
        let html = parsed
            .document
            .base
            .mutate()
            .create_element(html_element_name("html"), Vec::new());
        parsed
            .document
            .base
            .mutate()
            .append_children(document_root, &[html]);
        let head = parsed
            .document
            .base
            .mutate()
            .create_element(html_element_name("head"), Vec::new());
        parsed.document.base.mutate().append_children(html, &[head]);
        if let Some(title) = title {
            let title_text = title.to_string()?;
            let title_element = parsed
                .document
                .base
                .mutate()
                .create_element(html_element_name("title"), Vec::new());
            parsed
                .document
                .base
                .mutate()
                .append_children(head, &[title_element]);
            // Lone surrogates cannot survive the UTF-8 tree: they become the
            // replacement character at this boundary, a known cutover gap.
            let text = parsed.document.base.mutate().create_text_node(&title_text);
            parsed
                .document
                .base
                .mutate()
                .append_children(title_element, &[text]);
        }
        let body = parsed
            .document
            .base
            .mutate()
            .create_element(html_element_name("body"), Vec::new());
        parsed.document.base.mutate().append_children(html, &[body]);
        wrap_new_document_in_world(&ctx, parsed, &world_for_node(&ctx, self.document.0)?)
    }
}


/// Adopts `doctype` into `document` and appends it, retargeting the wrapper
/// so the caller's object and `document.doctype` are the same node
/// (<https://dom.spec.whatwg.org/#concept-node-adopt>).
fn append_adopted_doctype(ctx: &Ctx<'_>, document: NodeId, doctype: NodeId) -> Result<()> {
    let source_world = world_for_node(ctx, doctype)?;
    let snapshot = {
        let source = source_world.borrow();
        let Some(parsed) = source.document(doctype) else {
            return Err(rquickjs::Exception::throw_type(ctx, "no document"));
        };
        import_snapshot(&parsed.document, doctype, true)
            .ok_or_else(|| throw_dom(ctx, "HierarchyRequestError", "node cannot be adopted"))?
    };
    {
        let source = source_world.borrow();
        let Some(mut parsed) = source.document_mut(doctype) else {
            return Err(rquickjs::Exception::throw_type(ctx, "no document"));
        };
        detach_for_adopt(&mut parsed.document, doctype).map_err(|err| throw_dom_error(ctx, err))?;
    }
    let fresh = {
        let target_world = world_for_node(ctx, document)?;
        let target = target_world.borrow();
        let Some(mut parsed) = target.document_mut(document) else {
            return Err(rquickjs::Exception::throw_type(ctx, "no document"));
        };
        let fresh = materialize_import(&mut parsed.document, document.document, &snapshot)
            .map_err(|err| throw_dom_error(ctx, err))?;
        parsed
            .document
            .base
            .mutate()
            .append_children(document.node, &[fresh.node]);
        fresh
    };
    retarget_wrapper(ctx, doctype, fresh)
}

/// An HTML-namespace qualified name for document construction.
fn html_element_name(local: &str) -> QualName {
    QualName::new(
        None,
        crate::js::world::html_namespace(),
        LocalName::from(local),
    )
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
impl<'js> dom_parser_generated::DOMParser<'js> for JsDomParser {
    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-domparser
    fn constructor(ctx: &Ctx<'js>) -> Result<Self> {
        Ok(Self {
            url: world(ctx)?.borrow().document_url.as_str().to_owned(),
        })
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring
    fn parse_from_string(
        &self,
        ctx: Ctx<'js>,
        source: rquickjs::String<'js>,
        type_: dom_parser_generated::DOMParserSupportedType,
    ) -> Result<Value<'js>> {
        // A lone surrogate in the source is replaced with U+FFFD rather than
        // failing conversion: XML parsing observes the replacement, so the
        // document reports it in text
        // (<https://crbug.com/40814739>).
        let source = String::from_utf16_lossy(&source.to_utf16()?);
        let content_type = type_.as_str();
        // `DOMParser` parses with scripting disabled
        // (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
        // DOMParser documents never render: share fonts, skip UA sheets.
        let font_ctx = world(&ctx)?.borrow().runtime.font_ctx.clone();
        let base = self.url.parse::<url::Url>().map_or_else(
            |_| crate::render::INVALID_BASE_URL.to_owned(),
            |url| crate::render::blitz_base_url(&url),
        );
        let mut parsed = if content_type == "text/html" {
            let mut parsed = crate::parse_html(
                &source,
                blitz_dom::DocumentConfig {
                    base_url: Some(base),
                    font_ctx: Some(font_ctx),
                    ua_stylesheets: Some(Vec::new()),
                    ..blitz_dom::DocumentConfig::default()
                },
            );
            parsed.content_type = content_type;
            parsed.ready_state = crate::ReadyState::Complete;
            parsed
        } else {
            crate::xml::parse_document(&source, content_type, base, font_ctx)
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
pub struct JsXmlSerializer;

include!(concat!(env!("OUT_DIR"), "/XMLSerializer.rs"));

impl<'js> xml_serializer_generated::XMLSerializer<'js> for JsXmlSerializer {
    fn constructor(_ctx: &Ctx<'js>) -> Result<Self> {
        Ok(Self)
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-xmlserializer-serializetostring
    fn serialize_to_string(
        &self,
        ctx: Ctx<'js>,
        root: super::host::NodeReference,
    ) -> Result<rquickjs::String<'js>> {
        // An `Attr` serializes as the empty string
        // (<https://w3c.github.io/DOM-Parsing/#dfn-xml-serialization-algorithm>).
        let Some(id) = root.tree() else {
            return rquickjs::String::from_str(ctx.clone(), "");
        };
        let owner = world_for_node(&ctx, id)?;
        let text = owner.borrow().document(id).map_or_else(
            crate::dom_string::DomString::default,
            |parsed| {
                let mut output = super::node::HtmlOutput::default();
                super::node::serialize_xml_node(&parsed.document, id.node, &mut output);
                output.finish()
            },
        );
        crate::js::bindings::dom_string(&ctx, &text)
    }
}
