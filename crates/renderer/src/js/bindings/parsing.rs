//! `DOMImplementation`, DOM parsing, and serialization.

use super::{
    FromJs, JsAttr, LegacyNullString, NodeContext, OptString, OptionalTitle, WebIdlString,
    clone::{import_snapshot, materialize_import},
    create_kind, host_node_id, throw_dom, throw_dom_error, validate_and_extract, world,
    world_for_node, wrap_new_document,
};
use rquickjs::function::{Opt, Rest};

use dom::{LocalName, NodeId, NodeKind, QualName, html_namespace};

use rquickjs::{Class, Ctx, Exception, Result, Value, class::Trace};

use crate::js::world::Handle;

/// `DOMImplementation` as a platform object
/// (<https://dom.spec.whatwg.org/#interface-domimplementation>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMImplementation")]
pub struct JsImplementation {
    pub(crate) document: Handle,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; DOMImplementation methods ignore self"
)]
impl JsImplementation {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-hasfeature
    #[qjs(rename = "hasFeature")]
    fn has_feature(&self, _args: Rest<Value<'_>>) -> bool {
        true
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createdocumenttype
    #[qjs(rename = "createDocumentType")]
    fn create_document_type<'js>(
        &self,
        ctx: Ctx<'js>,
        name: String,
        public_id: String,
        system_id: String,
    ) -> Result<Value<'js>> {
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

    /// Creating a second document needs the renderer's World to own several
    /// trees; refused honestly until then.
    // https://dom.spec.whatwg.org/#dom-domimplementation-createdocument
    #[qjs(rename = "createDocument")]
    fn create_document<'js>(
        &self,
        ctx: Ctx<'js>,
        namespace: OptString,
        qualified: LegacyNullString,
        doctype: Value<'js>,
    ) -> Result<Value<'js>> {
        let namespace = namespace.0.unwrap_or_default();
        let content_type = match namespace.as_str() {
            "http://www.w3.org/1999/xhtml" => "application/xhtml+xml",
            "http://www.w3.org/2000/svg" => "image/svg+xml",
            _ => "application/xml",
        };
        // `qualifiedName` validates before the doctype steps run
        // (<https://dom.spec.whatwg.org/#dom-domimplementation-createdocument>).
        let root = if qualified.0.is_empty() {
            None
        } else {
            Some(validate_and_extract(
                &ctx,
                (!namespace.is_empty()).then_some(namespace.as_str()),
                &qualified.0,
                NodeContext::Element,
            )?)
        };
        let mut parsed = crate::Parsed::empty(content_type);
        let document = parsed.dom.document();
        if !doctype.is_null()
            && !doctype.is_undefined()
            && doctype_fields_for(&ctx, host_node_id(&ctx, &doctype).unwrap_or(document)).is_none()
        {
            return Err(Exception::throw_type(
                &ctx,
                "doctype argument is not a DocumentType",
            ));
        }
        // The argument node itself appends (adopted across arenas), keeping
        // wrapper identity with the passed doctype.
        if let Some(doctype) = host_node_id(&ctx, &doctype)
            && doctype_fields_for(&ctx, doctype).is_some()
        {
            let owner_rc = world_for_node(&ctx, doctype)?;
            let snapshot = {
                let owner = owner_rc.borrow();
                let Some(source) = owner.document(doctype) else {
                    return Err(Exception::throw_type(&ctx, "no document"));
                };
                import_snapshot(&source.dom, doctype, true)
            };
            if let Some(snapshot) = snapshot {
                let node = materialize_import(&mut parsed.dom, &snapshot)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
                parsed
                    .dom
                    .append(document, node)
                    .map_err(|err| throw_dom_error(&ctx, err))?;
            }
        }
        if let Some(name) = root {
            let element = parsed.dom.create_element(name, Vec::new());
            parsed
                .dom
                .append(document, element)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        wrap_new_document(&ctx, parsed)
    }

    // https://dom.spec.whatwg.org/#dom-domimplementation-createhtmldocument
    #[qjs(rename = "createHTMLDocument")]
    fn create_html_document<'js>(
        &self,
        ctx: Ctx<'js>,
        title: Opt<OptionalTitle>,
    ) -> Result<Value<'js>> {
        let mut parsed = crate::Parsed::empty("text/html");
        let document = parsed.dom.document();
        let doctype = parsed.dom.create_doctype("html", "", "");
        parsed
            .dom
            .append(document, doctype)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        let html = parsed
            .dom
            .create_element(html_element_name("html"), Vec::new());
        parsed
            .dom
            .append(document, html)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        let head = parsed
            .dom
            .create_element(html_element_name("head"), Vec::new());
        parsed
            .dom
            .append(html, head)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        if let Some(title) = title.0.and_then(|title| title.0) {
            let title_element = parsed
                .dom
                .create_element(html_element_name("title"), Vec::new());
            parsed
                .dom
                .append(head, title_element)
                .map_err(|err| throw_dom_error(&ctx, err))?;
            let text = parsed.dom.create_text(title);
            parsed
                .dom
                .append(title_element, text)
                .map_err(|err| throw_dom_error(&ctx, err))?;
        }
        let body = parsed
            .dom
            .create_element(html_element_name("body"), Vec::new());
        parsed
            .dom
            .append(html, body)
            .map_err(|err| throw_dom_error(&ctx, err))?;
        wrap_new_document(&ctx, parsed)
    }
}

/// The doctype's name, public id, and system id, when `parsed` holds `id`.
pub(super) fn doctype_fields(
    parsed: &crate::Parsed,
    id: NodeId,
) -> Option<(String, String, String)> {
    match parsed.dom.kind(id) {
        Some(NodeKind::Doctype {
            name,
            public_id,
            system_id,
        }) => Some((name.clone(), public_id.clone(), system_id.clone())),
        _ => None,
    }
}

/// [`doctype_fields`] for `id` in the current realm's world.
fn doctype_fields_for(ctx: &Ctx<'_>, id: NodeId) -> Option<(String, String, String)> {
    let world_rc = world(ctx).ok()?;
    let world = world_rc.borrow();
    let parsed = world.document(id)?;
    doctype_fields(&parsed, id)
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
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMParser")]
pub struct JsDomParser {
    /// The constructing realm's document URL. `parseFromString` parses with
    /// the context object's environment settings, not the caller's, so the
    /// instance remembers it; a directly-reached native cannot forge another
    /// document's URL either
    /// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
    url: String,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value"
)]
impl JsDomParser {
    #[qjs(constructor)]
    fn new(ctx: Ctx<'_>) -> Result<Self> {
        Ok(Self {
            url: world(&ctx)?.borrow().document_url.as_str().to_owned(),
        })
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring
    #[qjs(rename = "parseFromString")]
    fn parse_from_string<'js>(
        &self,
        ctx: Ctx<'js>,
        source: WebIdlString,
        type_: WebIdlString,
    ) -> Result<Value<'js>> {
        let content_type = CONTENT_TYPES
            .iter()
            .copied()
            .find(|valid| *valid == type_.0.as_str())
            .ok_or_else(|| {
                Exception::throw_type(
                    &ctx,
                    &format!("The provided value '{}' is not a valid enum value", type_.0),
                )
            })?;
        // `DOMParser` parses with scripting disabled
        // (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
        let mut parsed = if content_type == "text/html" {
            let mut parsed = crate::parse_html_without_scripting(&source.0);
            parsed.content_type = content_type;
            parsed.ready_state = crate::ReadyState::Complete;
            parsed
        } else {
            crate::xml::parse_document(&source.0, content_type)
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
pub(crate) const INSTALL_DOMPARSER_CTOR_JS: &str =
    include_str!("../scripts/parsing/dom_parser_ctor.js");

/// The `DOMParser` `parseFromString` `SupportedType` values
/// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
const CONTENT_TYPES: [&str; 5] = [
    "text/html",
    "text/xml",
    "application/xml",
    "application/xhtml+xml",
    "image/svg+xml",
];

/// `XMLSerializer` (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#xmlserializer>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "XMLSerializer")]
pub struct JsXmlSerializer {
    pub(crate) _reserved: Option<Handle>,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; XMLSerializer is a stateless constructor"
)]
impl JsXmlSerializer {
    #[qjs(constructor)]
    fn new() -> Self {
        Self { _reserved: None }
    }

    // https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-xmlserializer-serializetostring
    #[qjs(rename = "serializeToString")]
    fn serialize_to_string<'js>(&self, ctx: Ctx<'js>, root: Value<'js>) -> Result<String> {
        // An `Attr` serializes as the empty string
        // (<https://w3c.github.io/DOM-Parsing/#dfn-xml-serialization-algorithm>).
        if Class::<JsAttr>::from_js(&ctx, root.clone()).is_ok() {
            return Ok(String::new());
        }
        let Some(id) = host_node_id(&ctx, &root) else {
            return Err(Exception::throw_type(&ctx, "argument is not a Node"));
        };
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(id) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        crate::serialize::serialize_xml(&parsed.dom, id, false)
            .map_err(|err| throw_dom(&ctx, "InvalidStateError", &err.to_string()))
    }
}
