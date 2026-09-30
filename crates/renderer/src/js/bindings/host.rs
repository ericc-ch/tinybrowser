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

use super::world;

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
    Getter(Operation),
    Method { operation: Operation, length: usize },
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
            MemberKind::Getter(operation) => {
                let getter = Function::new_native(ctx.clone(), HostCall::new(dispatch, operation))?
                    .with_name(format!("get {}", member.name))?;
                prototype.prop(member.name, Getter(getter))?;
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

pub(crate) fn instance<'js, T: JsClass<'js>>(ctx: &Ctx<'js>, value: T) -> Result<Class<'js, T>> {
    let prototype = T::prototype(ctx)?
        .ok_or_else(|| Exception::throw_internal(ctx, "native interface has no prototype"))?;
    Class::instance_proto(value, prototype)
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

fn install_constants(target: &Object<'_>, constants: &[Constant]) -> Result<()> {
    // https://webidl.spec.whatwg.org/#define-the-constants
    for constant in constants {
        target.prop(constant.name, Property::from(constant.value).enumerable())?;
    }
    Ok(())
}

/// rquickjs's `Accessor` takes a generic Rust callback, not an existing JS
/// function. Supply that descriptor through its safe property trait instead.
struct Getter<'js>(Function<'js>);

impl<'js> AsProperty<'js, ()> for Getter<'js> {
    fn config(self, ctx: &Ctx<'js>) -> Result<(PropertyFlags, Value<'js>, Value<'js>, Value<'js>)> {
        use rquickjs::qjs;
        let flags = qjs::JS_PROP_HAS_GET
            | qjs::JS_PROP_HAS_ENUMERABLE
            | qjs::JS_PROP_ENUMERABLE
            | qjs::JS_PROP_HAS_CONFIGURABLE
            | qjs::JS_PROP_CONFIGURABLE;
        let flags = PropertyFlags::try_from(flags)
            .map_err(|_| Exception::throw_internal(ctx, "invalid native property flags"))?;
        Ok((
            flags,
            Value::new_undefined(ctx.clone()),
            self.0.into_value(),
            Value::new_undefined(ctx.clone()),
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
