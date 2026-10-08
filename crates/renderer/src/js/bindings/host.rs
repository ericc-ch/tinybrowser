//! Shared safe entry point and descriptor installation for generated bindings.
//!
//! Every member enters rquickjs with the same `HostCall` type. Interface-specific
//! behavior is a function pointer plus an operation tag, not a closure type per
//! member. `QuickJS` retains distinct JS function objects and original arguments.

use rquickjs::atom::PredefinedAtom;
use rquickjs::class::JsClass;
use rquickjs::function::{Constructor, NativeFunc, Params};
use rquickjs::object::{AsProperty, Property, PropertyFlags};
use rquickjs::{
    Class, Coerced, Ctx, Exception, FromJs, Function, Object, Persistent, Result, Value,
};

use super::{world, world_for_node};
use crate::js::world::NodeId;
pub(crate) use crate::js::world::NodeReference;

pub(crate) type Dispatch = for<'a, 'js> fn(Operation, &Params<'a, 'js>) -> Result<Value<'js>>;

/// A tag local to one generated interface's dispatch function.
#[derive(Clone, Copy)]
pub(crate) struct Operation(usize);

impl Operation {
    pub(crate) const fn new(index: usize) -> Self {
        Self(index)
    }

    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

pub(crate) struct Member {
    pub(crate) name: &'static str,
    pub(crate) kind: MemberKind,
}

#[derive(Clone, Copy)]
pub(crate) enum MemberKind {
    Attribute {
        getter: Operation,
        setter: Option<Operation>,
    },
    Method {
        operation: Operation,
        length: usize,
    },
}

pub(crate) struct Constant {
    pub(crate) name: &'static str,
    pub(crate) value: u16,
}

#[derive(Clone, Copy)]
pub(crate) struct HostCall {
    dispatch: Dispatch,
    operation: Operation,
}

impl HostCall {
    pub(crate) const fn new(dispatch: Dispatch, operation: Operation) -> Self {
        Self {
            dispatch,
            operation,
        }
    }
}

impl NativeFunc for HostCall {
    fn call<'js>(&self, params: Params<'_, 'js>) -> Result<Value<'js>> {
        (self.dispatch)(self.operation, &params)
    }
}

/// Install regular readonly attributes, constants, and the interface's tag.
pub(crate) fn prototype<'js>(
    ctx: &Ctx<'js>,
    name: &'static str,
    parent_intrinsic: Option<&str>,
    members: &[Member],
    constants: &[Constant],
    dispatch: Dispatch,
) -> Result<Object<'js>> {
    // https://webidl.spec.whatwg.org/#interface-prototype-object
    let realm = world(ctx)?;
    if let Some(prototype) = realm.borrow().brand(name) {
        return prototype.restore(ctx);
    }
    let prototype = Object::new(ctx.clone())?;
    if let Some(parent) = parent_intrinsic {
        let constructor: Object = ctx.globals().get(parent)?;
        let parent: Object = constructor.get("prototype")?;
        prototype.set_prototype(Some(&parent))?;
    }
    install_members(&prototype, members, constants, dispatch)?;
    // https://webidl.spec.whatwg.org/#interface-prototype-object
    prototype.prop(
        PredefinedAtom::SymbolToStringTag,
        Property::from(name).configurable(),
    )?;
    realm
        .borrow_mut()
        .intern_brand(name, Persistent::save(ctx, prototype.clone()));
    Ok(prototype)
}

pub(crate) fn install_members(
    prototype: &Object<'_>,
    members: &[Member],
    constants: &[Constant],
    dispatch: Dispatch,
) -> Result<()> {
    // https://webidl.spec.whatwg.org/#es-attributes
    // https://webidl.spec.whatwg.org/#es-operations
    let ctx = prototype.ctx();
    for member in members {
        match member.kind {
            MemberKind::Attribute { getter, setter } => {
                let getter = Function::new_native(ctx.clone(), HostCall::new(dispatch, getter))?
                    .with_name(format!("get {}", member.name))?;
                let setter = setter
                    .map(|operation| {
                        Function::new_native(ctx.clone(), HostCall::new(dispatch, operation))?
                            .with_name(format!("set {}", member.name))?
                            .with_length(1)
                    })
                    .transpose()?;
                prototype.prop(member.name, Attribute { getter, setter })?;
            }
            MemberKind::Method { operation, length } => {
                let function =
                    Function::new_native(ctx.clone(), HostCall::new(dispatch, operation))?
                        .with_name(member.name)?
                        .with_length(length)?;
                prototype.prop(
                    member.name,
                    Property::from(function)
                        .writable()
                        .enumerable()
                        .configurable(),
                )?;
            }
        }
    }
    install_constants(prototype, constants)
}

/// Define `@@unscopables` on the prototype, listing the `[Unscopable]`
/// members. Later installers on the same prototype merge rather than
/// replace, so two mixins can share one prototype
/// (<https://webidl.spec.whatwg.org/#es-operations>).
pub(crate) fn install_unscopables(prototype: &Object<'_>, names: &[&str]) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let ctx = prototype.ctx();
    let existing: Value = prototype.get(PredefinedAtom::SymbolUnscopables)?;
    let unscopables: Object = if existing.is_undefined() || existing.is_null() {
        Object::new(ctx.clone())?
    } else {
        existing
            .into_object()
            .ok_or_else(|| Exception::throw_internal(ctx, "@@unscopables is not an object"))?
    };
    for name in names {
        unscopables.set(*name, true)?;
    }
    prototype.prop(
        PredefinedAtom::SymbolUnscopables,
        Property::from(unscopables).configurable(),
    )
}

/// Read a reflected `DOMString` attribute: the content attribute value,
/// or the empty string when missing
/// (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#reflect>).
pub(crate) fn reflect_string<'js>(
    ctx: &Ctx<'js>,
    element: NodeId,
    name: &str,
) -> Result<rquickjs::String<'js>> {
    let world = super::world(ctx)?;
    let value = world
        .borrow()
        .document(element)
        .and_then(|parsed| {
            crate::js::world::attr(&parsed.document.base, element.node, name).map(str::to_owned)
        })
        .unwrap_or_default();
    rquickjs::String::from_str(ctx.clone(), &value)
}

/// Read a reflected `boolean` attribute: presence of the content attribute
/// (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#reflect>).
pub(crate) fn reflect_bool(ctx: &Ctx<'_>, element: NodeId, name: &str) -> Result<bool> {
    let world = super::world(ctx)?;
    Ok(world.borrow().document(element).is_some_and(|parsed| {
        crate::js::world::attr(&parsed.document.base, element.node, name).is_some()
    }))
}

/// Write a reflected `DOMString` attribute: set the content attribute and
/// notify mutation observers, mirroring the `setAttribute` algorithm.
pub(crate) fn reflect_set_string(
    ctx: &Ctx<'_>,
    element: NodeId,
    name: &str,
    value: &rquickjs::String<'_>,
) -> Result<()> {
    let value = value.to_string()?;
    super::set_attribute_sync(ctx, element, name, &value)
}

/// Write a reflected `boolean` attribute: present with the empty string
/// when true, removed when false.
pub(crate) fn reflect_set_bool(
    ctx: &Ctx<'_>,
    element: NodeId,
    name: &str,
    value: bool,
) -> Result<()> {
    if value {
        let empty = rquickjs::String::from_str(ctx.clone(), "")?;
        reflect_set_string(ctx, element, name, &empty)
    } else {
        super::remove_attribute_sync(ctx, element, "", name, false)
    }
}

/// Create a platform wrapper with the interface prototype from `ctx`'s realm
/// (<https://webidl.spec.whatwg.org/#dfn-platform-object>).
/// The rquickjs `Class::instance` prototype cache is shared across a runtime's
/// realms. Generated interface prototypes belong to one realm.
pub(crate) fn instance<'js, T: JsClass<'js>>(ctx: &Ctx<'js>, value: T) -> Result<Class<'js, T>> {
    let prototype = T::prototype(ctx)?
        .ok_or_else(|| Exception::throw_internal(ctx, "native interface has no prototype"))?;
    Class::instance_proto(value, prototype)
}

/// Create a node-associated platform wrapper with its document owner's
/// interface prototype (<https://dom.spec.whatwg.org/#concept-node>).
pub(crate) fn instance_for_node<'js, T: JsClass<'js>>(
    ctx: &Ctx<'js>,
    node: NodeId,
    value: T,
) -> Result<Class<'js, T>> {
    let owner = world_for_node(ctx, node)?;
    let prototype = owner
        .borrow()
        .brand(T::NAME)
        .ok_or_else(|| Exception::throw_internal(ctx, "node owner has no interface prototype"))?;
    Class::instance_proto(value, prototype.restore(ctx)?)
}

pub(crate) fn constructor<'js>(
    ctx: &Ctx<'js>,
    name: &str,
    length: usize,
    prototype: &Object<'js>,
    constants: &[Constant],
    callback: HostCall,
) -> Result<Constructor<'js>> {
    // https://webidl.spec.whatwg.org/#es-interface-call
    let function = Function::new_native(ctx.clone(), callback)?.with_constructor(true);
    function.prop("prototype", Property::from(prototype.clone()))?;
    prototype.prop(
        "constructor",
        Property::from(function.clone()).writable().configurable(),
    )?;
    let constructor = function.into_value().into_constructor().ok_or_else(|| {
        Exception::throw_internal(ctx, "native interface object is not a constructor")
    })?;
    constructor.set_name(name)?;
    constructor.set_length(length)?;
    install_constants(&constructor, constants)?;
    Ok(constructor)
}

pub(crate) fn constructor_prototype<'js, T: JsClass<'js>>(
    params: &Params<'_, 'js>,
) -> Result<Object<'js>> {
    // https://webidl.spec.whatwg.org/#internally-create-a-new-object-implementing-the-interface
    // Chromium/V8 and Firefox look up new.target's prototype before argument
    // conversion. This runs after conversion, in the order WebIDL specifies.
    let target = params
        .this()
        .into_function()
        .ok_or_else(|| Exception::throw_internal(params.ctx(), "constructor has no new.target"))?;
    let prototype: Value = target.get("prototype")?;
    if let Some(prototype) = prototype.into_object() {
        return Ok(prototype);
    }
    T::prototype(&target.realm()?)?
        .ok_or_else(|| Exception::throw_internal(params.ctx(), "native interface has no prototype"))
}

pub(crate) fn install_constants(target: &Object<'_>, constants: &[Constant]) -> Result<()> {
    // https://webidl.spec.whatwg.org/#define-the-constants
    for constant in constants {
        target.prop(constant.name, Property::from(constant.value).enumerable())?;
    }
    Ok(())
}

/// rquickjs's `Accessor` takes a generic Rust callback, not an existing JS
/// function. Supply that descriptor through its safe property trait instead.
struct Attribute<'js> {
    getter: Function<'js>,
    setter: Option<Function<'js>>,
}

impl<'js> AsProperty<'js, ()> for Attribute<'js> {
    fn config(self, ctx: &Ctx<'js>) -> Result<(PropertyFlags, Value<'js>, Value<'js>, Value<'js>)> {
        use rquickjs::qjs;
        let flags = qjs::JS_PROP_HAS_GET
            | qjs::JS_PROP_HAS_SET
            | qjs::JS_PROP_HAS_ENUMERABLE
            | qjs::JS_PROP_ENUMERABLE
            | qjs::JS_PROP_HAS_CONFIGURABLE
            | qjs::JS_PROP_CONFIGURABLE;
        let flags = PropertyFlags::try_from(flags)
            .map_err(|_| Exception::throw_internal(ctx, "invalid native property flags"))?;
        Ok((
            flags,
            Value::new_undefined(ctx.clone()),
            self.getter.into_value(),
            self.setter
                .map_or_else(|| Value::new_undefined(ctx.clone()), Function::into_value),
        ))
    }
}

pub(crate) fn require_constructor(params: &Params<'_, '_>) -> Result<()> {
    // https://webidl.spec.whatwg.org/#es-interface-call
    if params.is_constructor() {
        Ok(())
    } else {
        Err(Exception::throw_type(
            params.ctx(),
            "constructor requires new",
        ))
    }
}

pub(crate) fn require_arguments(params: &Params<'_, '_>, minimum: usize) -> Result<()> {
    // https://webidl.spec.whatwg.org/#dfn-overload-resolution-algorithm
    if params.len() >= minimum {
        Ok(())
    } else {
        Err(Exception::throw_type(
            params.ctx(),
            "missing required argument",
        ))
    }
}

pub(crate) fn receiver<'js, T: JsClass<'js>>(params: &Params<'_, 'js>) -> Result<Class<'js, T>> {
    // https://webidl.spec.whatwg.org/#es-attributes
    Class::from_value(&params.this())
        .map_err(|_| Exception::throw_type(params.ctx(), "incompatible receiver"))
}

pub(crate) fn node_argument<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<NodeReference> {
    // https://webidl.spec.whatwg.org/#js-interface
    // https://dom.spec.whatwg.org/#interface-attr
    if let Some(id) = super::host_node_id(ctx, value) {
        return Ok(NodeReference::Tree(id));
    }
    if let Ok(attr) = Class::<super::JsAttr>::from_value(value) {
        let attr = attr.borrow();
        return Ok(NodeReference::Attribute {
            scope: attr.scope.0,
            id: attr.id,
        });
    }
    Err(Exception::throw_type(ctx, "argument is not a Node"))
}

pub(crate) fn nullable_node_argument<'js>(
    ctx: &Ctx<'js>,
    value: &Value<'js>,
) -> Result<Option<NodeReference>> {
    if value.is_null() || value.is_undefined() {
        Ok(None)
    } else {
        node_argument(ctx, value).map(Some)
    }
}

/// A URL-reflected content attribute: absent reflects as the empty string,
/// otherwise the value resolves against the document base URL and
/// serializes, falling back to the raw value when parsing fails
/// (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#reflecting-content-attributes-in-idl-attributes>).
pub(crate) fn reflect_url_string(
    ctx: &Ctx<'_>,
    id: NodeId,
    content: &str,
) -> Result<crate::dom_string::DomString> {
    let world_rc = world(ctx)?;
    let raw: String = world_rc
        .borrow()
        .document(id)
        .and_then(|parsed| {
            crate::js::world::attr(&parsed.document.base, id.node, content).map(str::to_owned)
        })
        .unwrap_or_default();
    if raw.is_empty() {
        return Ok(crate::dom_string::DomString::default());
    }
    let base = super::document::document_base_url_string(ctx, id);
    Ok(url::Url::parse(&base)
        .ok()
        .and_then(|base| base.join(&raw).ok())
        .map_or(raw, |url| url.to_string())
        .into())
}

/// Validates an interface against native state on a shared payload.
/// Page-controlled prototypes must not grant an interface brand.
pub(crate) trait SharedClass {
    fn require_interface(&self, ctx: &Ctx<'_>, interface: &str) -> Result<()>;
}

/// Whether `data` implements the named node interface, or `None` when the
/// name is not one the shared payload can represent.
fn node_interface_matches(data: Option<&blitz_dom::NodeData>, interface: &str) -> Option<bool> {
    use blitz_dom::NodeData;
    Some(match interface {
        // Every node is also an `EventTarget`
        // (<https://dom.spec.whatwg.org/#interface-eventtarget>).
        "Node" | "EventTarget" => data.is_some(),
        "Document" | "XMLDocument" => matches!(data, Some(NodeData::Document(_))),
        // Blitz has no shadow roots; every fragment is a plain fragment.
        "DocumentFragment" | "ShadowRoot" => {
            matches!(data, Some(NodeData::Element(_)))
        }
        "Element" => matches!(data, Some(NodeData::Element(_))),
        "HTMLElement" => {
            matches!(data, Some(NodeData::Element(element)) if element.name.ns == crate::js::world::html_namespace())
        }
        "SVGElement" => {
            matches!(data, Some(NodeData::Element(element)) if element.name.ns == crate::js::world::svg_namespace())
        }
        "MathMLElement" => {
            matches!(data, Some(NodeData::Element(element)) if element.name.ns == crate::js::world::mathml_namespace())
        }
        "CharacterData" => matches!(data, Some(NodeData::Text(_) | NodeData::Comment { .. })),
        // Blitz has no doctype, PI, or CDATA nodes.
        "DocumentType" | "ProcessingInstruction" => false,
        // Spec mixins: their members are installed on every including
        // interface, so the receiver check accepts the union of those kinds.
        // `ElementCSSInlineStyle` is included by the HTML, SVG, and MathML
        // element interfaces.
        "ElementCSSInlineStyle" => matches!(
            data,
            Some(NodeData::Element(element))
                if element.name.ns == crate::js::world::html_namespace()
                    || element.name.ns == crate::js::world::svg_namespace()
                    || element.name.ns == crate::js::world::mathml_namespace()
        ),
        "ParentNode" => matches!(data, Some(NodeData::Document(_) | NodeData::Element(_))),
        "ChildNode" => matches!(
            data,
            Some(NodeData::Element(_) | NodeData::Text(_) | NodeData::Comment { .. })
        ),
        "NonDocumentTypeChildNode" => matches!(
            data,
            Some(NodeData::Element(_) | NodeData::Text(_) | NodeData::Comment { .. })
        ),
        // Per-element contracts check the element's local name. The hyperlink
        // mixin is included by the anchor and area interfaces.
        "HTMLHyperlinkElementUtils" => {
            crate::js::world::is_html_tag(data, "a") || crate::js::world::is_html_tag(data, "area")
        }
        // HTML element interfaces check the element's local name.
        "HTMLFormElement" => crate::js::world::is_html_tag(data, "form"),
        "HTMLInputElement" => crate::js::world::is_html_tag(data, "input"),
        "HTMLTextAreaElement" => crate::js::world::is_html_tag(data, "textarea"),
        "HTMLSelectElement" => crate::js::world::is_html_tag(data, "select"),
        "HTMLOptionElement" => crate::js::world::is_html_tag(data, "option"),
        "HTMLButtonElement" => crate::js::world::is_html_tag(data, "button"),
        "HTMLFieldSetElement" => crate::js::world::is_html_tag(data, "fieldset"),
        "HTMLOptGroupElement" => crate::js::world::is_html_tag(data, "optgroup"),
        "HTMLIFrameElement" => crate::js::world::is_html_tag(data, "iframe"),
        "HTMLFrameElement" => crate::js::world::is_html_tag(data, "frame"),
        "HTMLImageElement" => crate::js::world::is_html_tag(data, "img"),
        "HTMLCanvasElement" => crate::js::world::is_html_tag(data, "canvas"),
        "HTMLBaseElement" => crate::js::world::is_html_tag(data, "base"),
        "HTMLLinkElement" => crate::js::world::is_html_tag(data, "link"),
        "HTMLMediaElement" => {
            crate::js::world::is_html_tag(data, "audio")
                || crate::js::world::is_html_tag(data, "video")
        }
        "HTMLEmbedElement" => crate::js::world::is_html_tag(data, "embed"),
        "HTMLScriptElement" => crate::js::world::is_html_tag(data, "script"),
        "HTMLSourceElement" => crate::js::world::is_html_tag(data, "source"),
        "HTMLTrackElement" => crate::js::world::is_html_tag(data, "track"),
        "HTMLMetaElement" => crate::js::world::is_html_tag(data, "meta"),
        "HTMLMapElement" => crate::js::world::is_html_tag(data, "map"),
        "HTMLObjectElement" => crate::js::world::is_html_tag(data, "object"),
        "HTMLOutputElement" => crate::js::world::is_html_tag(data, "output"),
        "HTMLParamElement" => crate::js::world::is_html_tag(data, "param"),
        "HTMLSlotElement" => crate::js::world::is_html_tag(data, "slot"),
        "HTMLTemplateElement" => crate::js::world::is_html_tag(data, "template"),
        _ => return None,
    })
}

pub(crate) fn require_node_interface(ctx: &Ctx<'_>, id: NodeId, interface: &str) -> Result<()> {
    // https://webidl.spec.whatwg.org/#es-attributes
    // https://webidl.spec.whatwg.org/#es-operations
    let owner = super::world_for_node(ctx, id)?;
    let owner = owner.borrow();
    let document = owner
        .document(id)
        .ok_or_else(|| Exception::throw_type(ctx, "stale node"))?;
    let data = document
        .document
        .base
        .get_node(id.node)
        .map(|node| &node.data);
    match node_interface_matches(data, interface) {
        Some(true) => Ok(()),
        Some(false) => Err(Exception::throw_type(ctx, "incompatible receiver")),
        None => Err(Exception::throw_type(ctx, "unknown node interface")),
    }
}

/// Whether the JS value is a platform object implementing `interface`. The
/// non-throwing counterpart `require_node_interface` needs, used by union
/// conversion to try members in order.
pub(crate) fn is_interface<'js>(ctx: &Ctx<'js>, value: &Value<'js>, interface: &str) -> bool {
    let Some(id) = super::host_node_id(ctx, value) else {
        return false;
    };
    let Ok(owner) = super::world_for_node(ctx, id) else {
        return false;
    };
    let owner = owner.borrow();
    let Some(document) = owner.document(id) else {
        return false;
    };
    let data = document
        .document
        .base
        .get_node(id.node)
        .map(|node| &node.data);
    node_interface_matches(data, interface) == Some(true)
}

/// The receiver JS object for hand methods that keep it (observer identity
/// for callback delivery). The receiver check already passed, so a missing
/// object is an internal error, never a user throw.
pub(crate) fn this_object<'js>(params: &Params<'_, 'js>) -> Result<Object<'js>> {
    params
        .this()
        .into_object()
        .ok_or_else(|| Exception::throw_internal(params.ctx(), "native receiver has no object"))
}

pub(crate) fn put_forwards<'js>(
    params: &Params<'_, 'js>,
    name: &str,
    target: &str,
    nullable: bool,
) -> Result<Value<'js>> {
    // https://webidl.spec.whatwg.org/#es-attributes
    let ctx = params.ctx();
    let object = this_object(params)?;
    let forwarded: Value = object.get(name)?;
    // A nullable `[PutForwards]` target (`Document.location` on a detached
    // document) makes the assignment a no-op
    // (<https://webidl.spec.whatwg.org/#PutForwards>).
    if nullable && (forwarded.is_null() || forwarded.is_undefined()) {
        return Ok(Value::new_undefined(ctx.clone()));
    }
    let forwarded = forwarded
        .into_object()
        .ok_or_else(|| Exception::throw_type(ctx, "forwarded attribute is not an object"))?;
    let value = params
        .arg(0)
        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    let reflect_set = world(ctx)?
        .borrow()
        .pristine_reflect_set
        .clone()
        .ok_or_else(|| Exception::throw_internal(ctx, "Reflect.set was not captured"))?;
    let reflect_set = reflect_set.restore(ctx)?;
    let _: bool = reflect_set.call((forwarded, target, value))?;
    Ok(Value::new_undefined(ctx.clone()))
}

pub(crate) fn string_argument<'js>(
    params: &Params<'_, 'js>,
    index: usize,
    default: Option<&str>,
) -> Result<rquickjs::String<'js>> {
    // https://webidl.spec.whatwg.org/#dfn-overload-resolution-algorithm
    let value = params
        .arg(index)
        .unwrap_or_else(|| Value::new_undefined(params.ctx().clone()));
    if value.is_undefined()
        && let Some(default) = default
    {
        return rquickjs::String::from_str(params.ctx().clone(), default);
    }
    // https://webidl.spec.whatwg.org/#es-DOMString
    // Coerced uses the engine's ToString, not the mutable global String function
    // (whose special Symbol conversion also differs from WebIDL's ToString).
    Coerced::<rquickjs::String>::from_js(params.ctx(), value).map(|string| string.0)
}

/// A `boolean` argument, converted with the pristine `ToBoolean`
/// (<https://webidl.spec.whatwg.org/#es-boolean>).
pub(crate) fn boolean_argument(params: &Params<'_, '_>, index: usize) -> Result<bool> {
    let ctx = params.ctx();
    let value = params
        .arg(index)
        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    super::to_boolean(ctx, &value)
}

pub(crate) fn callback_argument<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Function<'js>> {
    // https://webidl.spec.whatwg.org/#js-callback-function
    value
        .clone()
        .into_function()
        .ok_or_else(|| Exception::throw_type(ctx, "argument is not a function"))
}

pub(crate) fn legacy_null_string_argument<'js>(
    params: &Params<'_, 'js>,
    index: usize,
) -> Result<rquickjs::String<'js>> {
    // https://webidl.spec.whatwg.org/#LegacyNullToEmptyString
    if params.arg(index).is_some_and(|value| value.is_null()) {
        rquickjs::String::from_str(params.ctx().clone(), "")
    } else {
        string_argument(params, index, None)
    }
}

pub(crate) fn document_type_argument<'js>(
    ctx: &Ctx<'js>,
    value: &Value<'js>,
) -> Result<Option<NodeReference>> {
    // https://webidl.spec.whatwg.org/#js-interface
    // https://webidl.spec.whatwg.org/#js-nullable-type
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    // Known gap (upstream, docs/progress.md): Blitz has no doctype nodes;
    // no value converts.
    let _ = super::required_node(ctx, value)?;
    Err(Exception::throw_type(ctx, "argument is not a DocumentType"))
}

pub(crate) fn nullable_string_argument<'js>(
    params: &Params<'_, 'js>,
    index: usize,
) -> Result<Option<rquickjs::String<'js>>> {
    // https://webidl.spec.whatwg.org/#js-nullable-type
    let value = params
        .arg(index)
        .unwrap_or_else(|| Value::new_undefined(params.ctx().clone()));
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    Coerced::<rquickjs::String>::from_js(params.ctx(), value).map(|string| Some(string.0))
}

/// One presence-preserving dictionary flag
/// (<https://webidl.spec.whatwg.org/#es-dictionary>): absent or `undefined`
/// is `None`, anything else converts with `ToBoolean` (`null` is false).
pub(crate) fn dict_flag<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    key: &str,
) -> Result<Option<bool>> {
    let value: Value = object.get(key)?;
    if value.is_undefined() {
        return Ok(None);
    }
    Coerced::<bool>::from_js(ctx, value).map(|flag| Some(flag.0))
}

/// A promise already resolved with `undefined`, for synchronous operations
/// whose IDL result is `Promise<undefined>`
/// (<https://webidl.spec.whatwg.org/#es-promise>).
pub(crate) fn resolved_promise<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
    let (promise, resolve, _) = ctx.promise()?;
    resolve.call::<_, ()>((Value::new_undefined(ctx.clone()),))?;
    Ok(promise.into_value())
}

/// One presence-preserving non-nullable interface dictionary member: a
/// present value must be an object; absent, null, and undefined throw
/// (<https://webidl.spec.whatwg.org/#es-interface>).
pub(crate) fn dict_required_object<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    key: &str,
) -> Result<Persistent<Object<'static>>> {
    let value: Value = object.get(key)?;
    let Some(object) = value.as_object() else {
        return Err(Exception::throw_type(
            ctx,
            "interface dictionary member must be an object",
        ));
    };
    Ok(Persistent::save(ctx, object.clone()))
}

/// One presence-preserving interface-typed dictionary member
/// (<https://webidl.spec.whatwg.org/#es-dictionary>): absent or `undefined` is
/// `None`. A present value must be an object
/// (<https://webidl.spec.whatwg.org/#es-interface>).
pub(crate) fn dict_object<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    key: &str,
) -> Result<Option<Persistent<Object<'static>>>> {
    let value: Value = object.get(key)?;
    // Only `undefined` means the member is absent; a present `null` is not an
    // object and fails the interface conversion
    // (<https://webidl.spec.whatwg.org/#es-interface>).
    if value.is_undefined() {
        return Ok(None);
    }
    let Some(object) = value.as_object() else {
        return Err(Exception::throw_type(
            ctx,
            "interface dictionary member must be an object",
        ));
    };
    Ok(Some(Persistent::save(ctx, object.clone())))
}

/// One presence-preserving string sequence
/// (<https://webidl.spec.whatwg.org/#js-sequence>): absent or `undefined` is
/// `None`. Present values must be objects with a callable iterator, whose
/// elements convert to `DOMString` without losing UTF-16 code units.
pub(crate) fn dict_string_sequence<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    key: &str,
) -> Result<Option<Vec<Vec<u16>>>> {
    let value: Value = object.get(key)?;
    if value.is_undefined() {
        return Ok(None);
    }
    let iterable = value
        .into_object()
        .ok_or_else(|| Exception::throw_type(ctx, "sequence must be an iterable object"))?;
    let method: Value = iterable.get(PredefinedAtom::SymbolIterator)?;
    let method = method
        .into_function()
        .ok_or_else(|| Exception::throw_type(ctx, "sequence iterator is not callable"))?;
    let iterator: Value = method.call((rquickjs::prelude::This(iterable),))?;
    let iterator = iterator
        .into_object()
        .ok_or_else(|| Exception::throw_type(ctx, "sequence iterator must return an object"))?;
    let next: Value = iterator.get("next")?;
    let next = next
        .into_function()
        .ok_or_else(|| Exception::throw_type(ctx, "sequence iterator next is not callable"))?;
    let mut items = Vec::new();
    loop {
        let result: Value = next.call((rquickjs::prelude::This(iterator.clone()),))?;
        let result = result.into_object().ok_or_else(|| {
            Exception::throw_type(ctx, "sequence iterator result must be an object")
        })?;
        let done: Coerced<bool> = result.get("done")?;
        if done.0 {
            break;
        }
        let value: Value = result.get("value")?;
        let string = Coerced::<rquickjs::String>::from_js(ctx, value)?.0;
        items.push(string.to_utf16()?);
    }
    Ok(Some(items))
}

/// A record sequence result as a fresh JS array
/// (<https://webidl.spec.whatwg.org/#es-sequence>).
pub(crate) fn sequence<'js>(ctx: &Ctx<'js>, values: Vec<Value<'js>>) -> Result<Value<'js>> {
    let array = rquickjs::Array::new(ctx.clone())?;
    for (index, value) in values.into_iter().enumerate() {
        array.as_object().prop(
            index.to_string(),
            Property::from(value).writable().enumerable().configurable(),
        )?;
    }
    Ok(array.into_value())
}

/// Reports an exception thrown by a host-invoked callback (timer, `fetch`,
/// mutation observer) through the invoking realm window's `error` event
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#report-the-error>).
/// The callback source carries no document line, so only the callback's own
/// stack location is used.
pub(crate) fn report_callback_error(ctx: &Ctx<'_>, error: &rquickjs::Error) {
    if error.is_exception() {
        let caught = ctx.catch();
        super::report_exception_value(ctx, caught, 0, "");
    }
}
