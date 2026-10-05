//! `DOMImplementation`, DOM parsing, and serialization.

use super::{
    NodeContext, detach_for_adopt, import_snapshot, materialize_import, throw_dom, throw_dom_error,
    validate_and_extract, world, world_for_node, wrap_new_document, wrap_new_document_in_world,
    wrap_node,
};

use crate::js::world::Handle;
use crate::js::world::NodeId;

use markup5ever::{LocalName, QualName};

use rquickjs::{Ctx, Exception, Result, Value, class::Trace};

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
        // Gap: Blitz `NodeData` has no Doctype variant, so a document type
        // cannot live in the tree. Materialize a Comment placeholder so the
        // wrapper stays live; `doctype_fields` reports `None` until a Doctype
        // kind exists.
        let owner = world_for_node(&ctx, self.document.0)?;
        let (document, blitz_id) = {
            let owner = owner.borrow();
            let Some(mut parsed) = owner.document_mut(self.document.0) else {
                return Err(Exception::throw_type(&ctx, "no document"));
            };
            let blitz_id =
                parsed
                    .document
                    .create_doctype(&name, &public_id, &system_id);
            (self.document.0.document, blitz_id)
        };
        wrap_node(
            &ctx,
            NodeId {
                document,
                node: blitz_id,
            },
        )
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
        let mut parsed = crate::Parsed::empty(content_type);
        // Adopt the doctype into the new tree when one was passed: snapshot
        // it out of its document, materialize the synthetic record locally,
        // and append it under the new root.
        if let Some(doctype) = doctype {
            let node = doctype.tree().ok_or_else(|| {
                throw_dom(&ctx, "HierarchyRequestError", "attributes cannot be adopted")
            })?;
            let snapshot = {
                let source_world = world_for_node(&ctx, node)?;
                let source = source_world.borrow();
                let Some(parsed) = source.document(node) else {
                    return Err(Exception::throw_type(&ctx, "no document"));
                };
                import_snapshot(&parsed.document, node, false).ok_or_else(|| {
                    throw_dom(&ctx, "HierarchyRequestError", "doctype cannot be adopted")
                })?
            };
            {
                let source_world = world_for_node(&ctx, node)?;
                let source = source_world.borrow();
                let Some(mut parsed) = source.document_mut(node) else {
                    return Err(Exception::throw_type(&ctx, "no document"));
                };
                detach_for_adopt(&mut parsed.document, node)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
            }
            let mut fresh = materialize_import(&mut parsed.document, &snapshot)
                .map_err(|err| throw_dom_error(&ctx, err))?;
            fresh.document = 0;
            let document_root = parsed.document.base.root_node().id;
            parsed
                .document
                .base
                .mutate()
                .append_children(document_root, &[fresh.node]);
        }
        if let Some(name) = root {
            let document_root = parsed.document.base.root_node().id;
            let element = parsed.document.base.mutate().create_element(name, Vec::new());
            parsed
                .document
                .base
                .mutate()
                .append_children(document_root, &[element]);
        }
        wrap_new_document_in_world(&ctx, parsed, &world_for_node(&ctx, self.document.0)?)
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createhtmldocument
    fn create_html_document(
        &self,
        ctx: Ctx<'js>,
        title: Option<rquickjs::String<'js>>,
    ) -> Result<Value<'js>> {
        let mut parsed = crate::Parsed::empty("text/html");
        // Gap: Blitz has no Doctype node kind; `createHTMLDocument` builds no
        // doctype, unlike the spec.
        let document_root = parsed.document.base.root_node().id;
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
        parsed
            .document
            .base
            .mutate()
            .append_children(html, &[head]);
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
            let text = parsed
                .document
                .base
                .mutate()
                .create_text_node(&title_text);
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
        parsed
            .document
            .base
            .mutate()
            .append_children(html, &[body]);
        wrap_new_document_in_world(&ctx, parsed, &world_for_node(&ctx, self.document.0)?)
    }
}

/// The doctype's name, public id, and system id, when `id` is a synthetic
/// doctype backing in `parsed`.
pub(super) fn doctype_fields(
    parsed: &crate::Parsed,
    id: NodeId,
) -> Option<(String, String, String)> {
    match parsed.document.synthetic_kind(id.node)? {
        crate::documents::SyntheticKind::Doctype {
            name,
            public_id,
            system_id,
        } => Some((name.clone(), public_id.clone(), system_id.clone())),
        _ => None,
    }
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
        let source = source.to_string()?;
        let content_type = type_.as_str();
        // `DOMParser` parses with scripting disabled
        // (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
        let mut parsed = if content_type == "text/html" {
            let base = self
                .url
                .parse::<url::Url>()
                .map_or_else(
                    |_| "http://invalid/".to_owned(),
                    |url| crate::render::blitz_base_url(&url),
                );
            let mut parsed = crate::parse_html_without_scripting(
                &source,
                blitz_dom::DocumentConfig {
                    base_url: Some(base),
                    ..blitz_dom::DocumentConfig::default()
                },
            );
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

impl<'js> xml_serializer_generated::XMLSerializer<'js> for JsXmlSerializer {
    fn constructor(_ctx: &Ctx<'js>) -> Result<Self> {
        Ok(Self { _reserved: None })
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-xmlserializer-serializetostring
    fn serialize_to_string(
        &self,
        ctx: Ctx<'js>,
        root: super::host::NodeReference,
    ) -> Result<rquickjs::String<'js>> {
        // An `Attr` serializes as the empty string
        // (<https://w3c.github.io/DOM-Parsing/#dfn-xml-serialization-algorithm>).
        // Gap: the old serializer still targets the previous tree type
        // and has no Blitz equivalent yet; every tree node serializes as the
        // empty string until the serializer is ported.
        let _ = matches!(root, super::host::NodeReference::Tree(_));
        rquickjs::String::from_str(ctx.clone(), "")
    }
}
