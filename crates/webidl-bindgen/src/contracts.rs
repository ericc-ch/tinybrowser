use std::collections::{BTreeMap, BTreeSet, HashSet};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use weedle::argument::Argument;
use weedle::attribute::{ExtendedAttribute, ExtendedAttributeList};
use weedle::interface::{InterfaceMember, Special};
use weedle::literal::DefaultValue;
use weedle::types::{IntegerType, NonAnyType, SingleType, Type};

use crate::database::Database;
use crate::model::{
    self, ArgumentArity, ConstructorArgumentKind, CtxMode, GetterMapping, InterfaceKind,
    OperationResult, PropertyGetter, PropertyHooks, PrototypeParent, ReturnType,
};
use crate::names::snake_case;
use crate::{Binding, Error, Source};

/// One discovered implementation method: its parameter count after `self` and
/// `ctx`, and whether the first such parameter is the JS receiver object.
#[derive(Clone, Copy)]
struct Method {
    has_self: bool,
    parameters: usize,
    receiver: bool,
}

impl Method {
    fn parse(member: &syn::ImplItemFn, source: &str) -> Result<Self, Error> {
        let inputs = &member.sig.inputs;
        let (ctx_index, has_self) = match inputs.first() {
            Some(syn::FnArg::Receiver(_)) => (1, true),
            Some(syn::FnArg::Typed(_)) => (0, false),
            None => {
                return Err(Error(format!(
                    "{source}: binding methods take a Ctx parameter first"
                )));
            }
        };
        let Some(syn::FnArg::Typed(ctx)) = inputs.get(ctx_index) else {
            return Err(Error(format!(
                "{source}: binding methods take a Ctx parameter first"
            )));
        };
        let is_ctx = matches!(&*ctx.ty, syn::Type::Reference(reference) if is_ctx_type(&reference.elem))
            || is_ctx_type(&ctx.ty);
        if !is_ctx {
            return Err(Error(format!(
                "{source}: binding methods take a Ctx parameter first"
            )));
        }
        let rest: Vec<_> = inputs.iter().skip(ctx_index + 1).collect();
        let receiver = rest.first().is_some_and(|argument| {
            matches!(argument, syn::FnArg::Typed(argument) if is_object_type(&argument.ty))
        });
        Ok(Self {
            has_self,
            parameters: rest.len(),
            receiver,
        })
    }
}

fn is_ctx_type(type_: &syn::Type) -> bool {
    matches!(type_, syn::Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Ctx"))
}

fn is_object_type(type_: &syn::Type) -> bool {
    match type_ {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Object"),
        // `Object<'js>` inside a reference is not a receiver.
        _ => false,
    }
}

struct Implementation {
    interface: String,
    payload: syn::Ident,
    lifetime: bool,
    methods: BTreeMap<String, Method>,
}

pub(crate) fn compile(idl: &[Source<'_>], rust: &[Source<'_>]) -> Result<Vec<Binding>, Error> {
    let database = Database::parse(idl)?;
    let mut implementations = BTreeMap::new();
    let mut classes = BTreeSet::new();
    for source in rust {
        let syntax = syn::parse_file(source.text)
            .map_err(|error| Error(format!("{}: invalid Rust: {error}", source.name)))?;
        discover(
            &syntax.items,
            source.name,
            &mut implementations,
            &mut classes,
        )?;
    }
    implementations
        .into_values()
        .map(|implementation| {
            let kind = if classes.contains(&implementation.payload.to_string()) {
                InterfaceKind::Partial
            } else {
                InterfaceKind::Complete
            };
            let interface = lower(&database, &implementation, kind)?;
            let syntax = syn::parse2(crate::emit::interface(&interface))
                .map_err(|error| Error(format!("invalid generated contract: {error}")))?;
            Ok(Binding {
                interface: implementation.interface,
                rust: prettyplease::unparse(&syntax),
            })
        })
        .collect()
}

fn discover(
    items: &[syn::Item],
    source: &str,
    implementations: &mut BTreeMap<String, Implementation>,
    classes: &mut BTreeSet<String>,
) -> Result<(), Error> {
    for item in items {
        if let syn::Item::Mod(module) = item
            && let Some((_, items)) = &module.content
        {
            discover(items, source, implementations, classes)?;
        }
        if let syn::Item::Struct(item) = item
            && item.attrs.iter().any(|attribute| {
                attribute
                    .path()
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .eq(["rquickjs", "class"].map(String::from))
            })
        {
            if item.attrs.iter().any(|attribute| {
                attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr")
            }) {
                return Err(Error(format!(
                    "{source}: native classes must not conditionally disappear"
                )));
            }
            classes.insert(item.ident.to_string());
        }
        let syn::Item::Impl(item) = item else {
            continue;
        };
        let Some(implementation) = Implementation::parse(item, source)? else {
            continue;
        };
        let name = implementation.interface.clone();
        if implementations
            .insert(name.clone(), implementation)
            .is_some()
        {
            return Err(Error(format!(
                "{source}: multiple native implementations of {name} are not supported"
            )));
        }
    }
    Ok(())
}

impl Implementation {
    fn parse(item: &syn::ItemImpl, source: &str) -> Result<Option<Self>, Error> {
        let Some((path, _)) = &item.trait_ else {
            return Ok(None);
        };
        let segments: Vec<_> = path.segments.iter().collect();
        let [module, interface] = segments.as_slice() else {
            return Ok(None);
        };
        if !module.ident.to_string().ends_with("_generated") {
            return Ok(None);
        }
        let name = interface.ident.to_string();
        if module.ident != format!("{}_generated", snake_case(&name)) {
            return Err(Error(format!(
                "{source}: generated contract module must follow the interface name {name}"
            )));
        }
        if item.modifiers.polarity.is_some()
            || item.modifiers.defaultness.is_some()
            || item.unsafety.is_some()
            || item.attrs.iter().any(|attribute| {
                attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr")
            })
        {
            return Err(Error(format!(
                "{source}: binding implementations must be unconditional safe trait implementations"
            )));
        }
        let syn::Type::Path(payload) = &*item.self_ty else {
            return Err(Error(format!(
                "{source}: binding payload must be a named local type"
            )));
        };
        let payload = payload_type(&payload.path).ok_or_else(|| {
            Error(format!(
                "{source}: binding payload must be a local type with at most a 'js lifetime"
            ))
        })?;
        let mut methods = BTreeMap::new();
        for member in &item.items {
            let syn::ImplItem::Fn(member) = member else {
                return Err(Error(format!(
                    "{source}: binding implementations contain only platform algorithm methods"
                )));
            };
            if member.attrs.iter().any(|attribute| {
                attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr")
            }) {
                return Err(Error(format!(
                    "{source}: binding methods must not conditionally disappear"
                )));
            }
            let method = Method::parse(member, source)?;
            if methods
                .insert(member.sig.ident.to_string(), method)
                .is_some()
            {
                return Err(Error(format!(
                    "{source}: duplicate binding method {}",
                    member.sig.ident
                )));
            }
        }
        Ok(Some(Self {
            interface: name,
            payload: payload.0,
            lifetime: payload.1,
            methods,
        }))
    }
}

fn payload_type(path: &syn::Path) -> Option<(syn::Ident, bool)> {
    if path.leading_colon.is_some() || path.segments.len() != 1 {
        return None;
    }
    let segment = path.segments.first()?;
    match &segment.arguments {
        syn::PathArguments::None => Some((segment.ident.clone(), false)),
        syn::PathArguments::AngleBracketed(arguments)
            if arguments.args.len() == 1
                && matches!(arguments.args.first(), Some(syn::GenericArgument::Lifetime(lifetime)) if lifetime.ident == "js") =>
        {
            Some((segment.ident.clone(), true))
        }
        _ => None,
    }
}

fn lower(
    database: &Database<'_>,
    implementation: &Implementation,
    kind: InterfaceKind,
) -> Result<model::Interface, Error> {
    let declaration = database.interface(&implementation.interface)?;
    validate_interface_attributes(declaration.attributes.as_ref())?;
    // https://webidl.spec.whatwg.org/#js-DOMException-specialness
    let parent = declaration
        .parent
        .map(|parent| PrototypeParent::Interface(parent.into()))
        .or_else(|| {
            (declaration.name == "DOMException").then(|| PrototypeParent::Intrinsic("Error".into()))
        });
    let mut interface = model::Interface {
        name: declaration.name.into(),
        rust: implementation.payload.clone(),
        alternate: None,
        alternate_has_lifetime: false,
        has_lifetime: implementation.lifetime,
        parent,
        constructor: None,
        attributes: Vec::new(),
        constants: Vec::new(),
        kind,
        operations: Vec::new(),
        dictionaries: Vec::new(),
        enumerations: Vec::new(),
        properties: PropertyHooks::None,
        stringifier: None,
        value_iterable: false,
        indexed_setter: None,
        install_targets: Vec::new(),
        ctx_mode: CtxMode::Borrowed,
        contract: None,
    };
    interface.contract = Some(lower_members(
        database,
        &declaration,
        implementation,
        &mut interface,
    )?);
    if matches!(interface.kind, InterfaceKind::Partial)
        && (interface.constructor.is_some() || !matches!(interface.properties, PropertyHooks::None))
    {
        return Err(Error(
            "bindings for existing classes cannot replace constructors or property hooks".into(),
        ));
    }
    add_referenced_definitions(database, &mut interface)?;
    Ok(interface)
}

fn lower_members(
    database: &Database<'_>,
    declaration: &crate::database::Interface<'_>,
    implementation: &Implementation,
    interface: &mut model::Interface,
) -> Result<TokenStream, Error> {
    let mut remaining = implementation.methods.clone();
    let mut methods = Vec::new();
    let mut getters = Vec::new();
    for member in &declaration.members {
        let resolved = member;
        match &member.declaration {
            InterfaceMember::Const(member) => {
                resolved.validate_scopes()?;
                interface.constants.push(model::Constant::parse(member)?);
            }
            InterfaceMember::Constructor(member)
                if implementation.methods.contains_key("constructor") =>
            {
                resolved.validate_scopes()?;
                if interface.constructor.is_some() {
                    return Err(Error(
                        "native constructor overloads are not supported yet".into(),
                    ));
                }
                remaining.remove("constructor");
                if implementation.methods["constructor"].has_self {
                    return Err(Error(
                        "native constructors are associated functions, not methods".into(),
                    ));
                }
                let (constructor, signature) = lower_constructor(database, member)?;
                interface.constructor = Some(constructor);
                methods.push(signature);
            }
            InterfaceMember::Attribute(member) => {
                let Some((attribute, signature)) =
                    lower_attribute(database, member, &implementation.methods)?
                else {
                    continue;
                };
                resolved.validate_scopes()?;
                remaining.remove(&attribute.rust.to_string());
                if let Some(model::Setter::Method { rust, .. }) = &attribute.setter {
                    remaining.remove(&rust.to_string());
                }
                if matches!(
                    member.modifier,
                    Some(weedle::interface::StringifierOrInheritOrStatic::Stringifier(_))
                ) && interface
                    .stringifier
                    .replace(interface.attributes.len())
                    .is_some()
                {
                    return Err(Error("multiple stringifiers are not supported".into()));
                }
                methods.push(signature);
                interface.attributes.push(attribute);
            }
            InterfaceMember::Operation(member) => {
                let Some((operation, signature, property)) =
                    lower_operation(database, member, &implementation.methods)?
                else {
                    continue;
                };
                resolved.validate_scopes()?;
                remaining.remove(&operation.rust.to_string());
                if let Some(property) = property {
                    getters.push(property);
                }
                methods.push(signature);
                interface.operations.push(operation);
            }
            InterfaceMember::Constructor(_)
            | InterfaceMember::Iterable(_)
            | InterfaceMember::AsyncIterable(_)
            | InterfaceMember::Maplike(_)
            | InterfaceMember::Setlike(_)
            | InterfaceMember::Stringifier(_) => {}
        }
    }
    interface.properties =
        infer_property_hooks(declaration, &getters, &mut remaining, &mut methods)?;
    ensure_no_extra_methods(declaration, &remaining)?;
    let name = format_ident!("{}", declaration.name);
    Ok(quote! { pub(super) trait #name<'js> { #(#methods)* } })
}

/// Reject implementation methods that no IDL member consumed.
fn ensure_no_extra_methods(
    declaration: &crate::database::Interface<'_>,
    remaining: &BTreeMap<String, Method>,
) -> Result<(), Error> {
    if remaining.is_empty() {
        return Ok(());
    }
    Err(Error(format!(
        "{}: methods do not match a supported IDL contract: {}",
        declaration.name,
        remaining.keys().cloned().collect::<Vec<_>>().join(", ")
    )))
}

/// Emit the dictionaries and enumerations the lowered members reference.
fn add_referenced_definitions(
    database: &Database<'_>,
    interface: &mut model::Interface,
) -> Result<(), Error> {
    let mut referenced = BTreeSet::new();
    for attribute in &interface.attributes {
        collect_reference(&attribute.return_type, &mut referenced);
    }
    for operation in &interface.operations {
        for argument in &operation.arguments {
            collect_reference(&argument.type_, &mut referenced);
        }
    }
    if let Some(constructor) = &interface.constructor {
        for argument in &constructor.arguments {
            if let ConstructorArgumentKind::Dictionary(name) = &argument.kind {
                referenced.insert(name.clone());
            }
        }
    }
    for name in referenced {
        match database.definition(&name) {
            Some(weedle::Definition::Dictionary(_)) => {
                interface
                    .dictionaries
                    .push(lower_dictionary(database, &name)?);
            }
            Some(weedle::Definition::Enum(definition)) => {
                interface
                    .enumerations
                    .push(model::Enumeration::parse(definition)?);
            }
            Some(weedle::Definition::Callback(_)) => {}
            _ => {
                return Err(Error(format!(
                    "native reference {name} is not supported yet"
                )));
            }
        }
    }
    Ok(())
}

/// Note a dictionary or enumeration a lowered member references, so its
/// definition is emitted into the same generated module.
fn collect_reference(type_: &ReturnType, referenced: &mut BTreeSet<String>) {
    match type_ {
        ReturnType::Dictionary(name) | ReturnType::Enumeration(name) => {
            referenced.insert(name.clone());
        }
        _ => {}
    }
}

/// Lower an unchanged dictionary declaration. Field names are the IDL names
/// snake-cased; no `Rust` annotations are read.
fn lower_dictionary(database: &Database<'_>, name: &str) -> Result<model::Dictionary, Error> {
    let definition = database.dictionary(name)?;
    validate_dictionary_attributes(definition.attributes.as_ref())?;
    let inherited = if let Some(parent) = &definition.inheritance {
        lower_dictionary(database, parent.identifier.0)?.fields
    } else {
        Vec::new()
    };
    let mut fields = Vec::new();
    let mut taken: HashSet<_> = inherited.iter().map(|field| field.name.as_str()).collect();
    for member in &definition.members.body {
        if member.required.is_some() {
            return Err(Error(
                "required dictionary fields are not supported yet".into(),
            ));
        }
        if !taken.insert(member.identifier.0) {
            return Err(Error(format!(
                "duplicate dictionary field: {}",
                member.identifier.0
            )));
        }
        validate_empty_attributes(member.attributes.as_ref())?;
        validate_empty_attributes(member.type_.attributes.as_ref())?;
        let rust = format_ident!("{}", snake_case(member.identifier.0));
        let type_ = match &member.type_.type_ {
            Type::Single(SingleType::NonAny(NonAnyType::Boolean(value)))
                if value.q_mark.is_none() =>
            {
                let default = match &member.default {
                    None => None,
                    Some(default) => {
                        let DefaultValue::Boolean(default) = &default.value else {
                            return Err(Error(
                                "only boolean dictionary defaults are supported yet".into(),
                            ));
                        };
                        Some(default.0)
                    }
                };
                model::DictionaryFieldType::Boolean { default }
            }
            Type::Single(SingleType::NonAny(NonAnyType::Sequence(value)))
                if value.q_mark.is_none() =>
            {
                let element = &value.type_.generics.body;
                validate_empty_attributes(element.attributes.as_ref())?;
                let is_string = matches!(
                    &element.type_,
                    Type::Single(SingleType::NonAny(NonAnyType::DOMString(element)))
                        if element.q_mark.is_none()
                );
                if !is_string {
                    return Err(Error(
                        "only sequence<DOMString> dictionary fields are supported yet".into(),
                    ));
                }
                if member.default.is_some() {
                    return Err(Error(
                        "sequence dictionary defaults are not supported yet".into(),
                    ));
                }
                model::DictionaryFieldType::StringSequence
            }
            _ => {
                return Err(Error(format!(
                    "dictionary field type is not supported yet: {}",
                    member.identifier.0
                )));
            }
        };
        fields.push(model::DictionaryField {
            name: member.identifier.0.into(),
            rust,
            type_,
        });
    }
    // https://webidl.spec.whatwg.org/#es-dictionary
    // https://github.com/chromium/chromium/blob/main/third_party/blink/renderer/bindings/scripts/web_idl/dictionary.py
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    let fields = inherited.into_iter().chain(fields).collect();
    Ok(model::Dictionary {
        name: definition.identifier.0.into(),
        fields,
    })
}

/// Dictionary-level extended attributes the contract path consumes.
fn validate_dictionary_attributes(
    attributes: Option<&ExtendedAttributeList<'_>>,
) -> Result<(), Error> {
    if attributes.is_some_and(|attributes| !attributes.body.list.is_empty()) {
        Err(Error(format!(
            "native dictionary semantics are not supported yet: {attributes:?}"
        )))
    } else {
        Ok(())
    }
}

/// Infer legacy platform-object property hooks from declared getters.
fn infer_property_hooks(
    declaration: &crate::database::Interface<'_>,
    getters: &[PropertyGetter],
    remaining: &mut BTreeMap<String, Method>,
    methods: &mut Vec<TokenStream>,
) -> Result<PropertyHooks, Error> {
    let indexed = getters.contains(&PropertyGetter::Indexed);
    let named = getters.contains(&PropertyGetter::Named);
    match (indexed, named) {
        (false, false) => Ok(PropertyHooks::None),
        (true, false) => Ok(PropertyHooks::Indexed),
        (false, true) => Err(Error(
            "named-only property hooks are not supported yet".into(),
        )),
        (true, true) => {
            let supported = format_ident!("supported_names");
            if remaining.remove("supported_names").is_none() {
                return Err(Error(
                    "named property hooks require a supported_names implementation".into(),
                ));
            }
            methods
                .push(quote! { fn supported_names(&self, ctx: &Ctx<'js>) -> Result<Vec<String>>; });
            let attributes = declaration.attributes.as_ref();
            Ok(PropertyHooks::IndexedNamed {
                names: supported,
                unenumerable: has_attribute(attributes, "LegacyUnenumerableNamedProperties"),
                override_builtins: has_attribute(attributes, "LegacyOverrideBuiltIns"),
            })
        }
    }
}

fn has_attribute(
    attributes: Option<&weedle::attribute::ExtendedAttributeList<'_>>,
    name: &str,
) -> bool {
    attributes.is_some_and(|attributes| {
        attributes
            .body
            .list
            .iter()
            .any(|attribute| match attribute {
                weedle::attribute::ExtendedAttribute::NoArgs(attribute) => attribute.0.0 == name,
                _ => false,
            })
    })
}

fn lower_operation(
    database: &Database<'_>,
    member: &weedle::interface::OperationInterfaceMember<'_>,
    implemented: &BTreeMap<String, Method>,
) -> Result<Option<(model::Operation, TokenStream, Option<PropertyGetter>)>, Error> {
    let Some(identifier) = &member.identifier else {
        return Ok(None);
    };
    let name = identifier.0;
    let rust = format_ident!("{}", snake_case(name));
    let Some(method) = implemented.get(&rust.to_string()) else {
        return Ok(None);
    };
    if member.modifier.is_some() {
        return Err(Error(
            "native static and stringifier operations are not supported yet".into(),
        ));
    }
    validate_operation_attributes(member.attributes.as_ref())?;
    let reactions = has_attribute(member.attributes.as_ref(), "CEReactions");
    if !method.has_self {
        return Err(Error(format!(
            "native operation {name} must take self as its first parameter"
        )));
    }
    let property = match &member.special {
        None => None,
        Some(Special::Getter(_)) => Some(property_getter(database, member)?),
        Some(_) => {
            return Err(Error(
                "only readonly property getters are supported yet".into(),
            ));
        }
    };
    let mut arguments = Vec::new();
    let mut parameters = Vec::new();
    let mut optional_seen = false;
    for (index, argument) in member.args.body.list.iter().enumerate() {
        let LoweredArgument {
            argument,
            parameter,
            arity,
        } = lower_argument(database, argument)?;
        if arity == ArgumentArity::Variadic && index + 1 != member.args.body.list.len() {
            return Err(Error("variadic arguments must be last".into()));
        }
        if optional_seen && arity == ArgumentArity::Required {
            return Err(Error(
                "native required argument after an optional one is not supported yet".into(),
            ));
        }
        optional_seen |= arity == ArgumentArity::Optional;
        let argument_name = format_ident!("arg_{index}");
        parameters.push(quote! { #argument_name: #parameter });
        arguments.push(argument);
    }
    let expected = arguments.len();
    let takes_this = if method.parameters == expected {
        false
    } else if method.parameters == expected + 1 && method.receiver && property.is_none() {
        true
    } else {
        return Err(Error(format!(
            "native operation {name} has an implementation signature that does not match its IDL arguments"
        )));
    };
    let this_parameter = if takes_this {
        quote! { this: Object<'js>, }
    } else {
        quote! {}
    };
    let (result, returns) = operation_result(database, &member.return_type)?;
    let signature = quote! {
        fn #rust(&self, ctx: Ctx<'js>, #this_parameter #(#parameters),*) -> Result<#returns>;
    };
    let operation = model::Operation {
        name: name.into(),
        rust,
        result,
        takes_this,
        arguments,
        reactions,
        getter: property,
    };
    Ok(Some((operation, signature, property)))
}

struct LoweredArgument {
    argument: model::OperationArgument,
    parameter: TokenStream,
    arity: ArgumentArity,
}

fn lower_argument(
    database: &Database<'_>,
    argument: &weedle::argument::Argument<'_>,
) -> Result<LoweredArgument, Error> {
    let argument = match argument {
        Argument::Single(argument) => argument,
        Argument::Variadic(argument) => {
            validate_empty_attributes(argument.attributes.as_ref())?;
            let type_ = native_type(database, &argument.type_, &mut BTreeSet::new())?;
            let element = argument_parameter(&type_, ArgumentArity::Required, None)?;
            return Ok(LoweredArgument {
                argument: model::OperationArgument {
                    type_,
                    arity: ArgumentArity::Variadic,
                    null_default: false,
                    legacy_null_to_empty: false,
                    boolean_default: None,
                    from_js: None,
                },
                parameter: quote! { Vec<#element> },
                arity: ArgumentArity::Variadic,
            });
        }
    };
    validate_argument_attributes(argument.attributes.as_ref())?;
    validate_argument_attributes(argument.type_.attributes.as_ref())?;
    let legacy_null_to_empty =
        has_attribute(argument.attributes.as_ref(), "LegacyNullToEmptyString")
            || has_attribute(
                argument.type_.attributes.as_ref(),
                "LegacyNullToEmptyString",
            );
    let type_ = native_type(database, &argument.type_.type_, &mut BTreeSet::new())?;
    if legacy_null_to_empty && !matches!(type_, ReturnType::String) {
        return Err(Error("LegacyNullToEmptyString requires DOMString".into()));
    }
    // https://webidl.spec.whatwg.org/#dfn-overload-resolution-algorithm
    match (&type_, argument.optional.is_some(), &argument.default) {
        (_, false, None) | (ReturnType::String | ReturnType::Boolean, true, None) => {}
        (ReturnType::Boolean, true, Some(default))
            if matches!(default.value, DefaultValue::Boolean(_)) => {}
        (ReturnType::NullableDocumentType, true, Some(default))
            if matches!(default.value, DefaultValue::Null(_)) => {}
        (ReturnType::Dictionary(_), true, Some(default))
            if matches!(default.value, DefaultValue::EmptyDictionary(_)) => {}
        _ => {
            return Err(Error(
                "native operation optionality or default is not supported yet".into(),
            ));
        }
    }
    let arity = if argument.optional.is_some() {
        ArgumentArity::Optional
    } else {
        ArgumentArity::Required
    };
    let boolean_default = argument
        .default
        .as_ref()
        .and_then(|default| match default.value {
            DefaultValue::Boolean(value) => Some(value.0),
            _ => None,
        });
    let parameter = argument_parameter(&type_, arity, boolean_default)?;
    Ok(LoweredArgument {
        argument: model::OperationArgument {
            type_,
            arity,
            null_default: argument
                .default
                .as_ref()
                .is_some_and(|default| matches!(default.value, DefaultValue::Null(_))),
            legacy_null_to_empty,
            boolean_default,
            from_js: None,
        },
        parameter,
        arity,
    })
}

/// The Rust parameter type for one lowered operation argument. These mirror the
/// conversions in `emit`, and match the payload signatures the platform
/// algorithms already use.
fn argument_parameter(
    type_: &ReturnType,
    arity: ArgumentArity,
    boolean_default: Option<bool>,
) -> Result<TokenStream, Error> {
    let optional = arity == ArgumentArity::Optional;
    Ok(match type_ {
        ReturnType::Node => quote! { host::NodeReference },
        ReturnType::NullableNode => quote! { Option<host::NodeReference> },
        ReturnType::String if optional => quote! { Option<rquickjs::String<'js>> },
        ReturnType::String => quote! { rquickjs::String<'js> },
        ReturnType::NullableString => quote! { Option<rquickjs::String<'js>> },
        ReturnType::NullableDocumentType => quote! { Option<dom::NodeId> },
        ReturnType::Callback => quote! { rquickjs::Function<'js> },
        ReturnType::Dictionary(name) | ReturnType::Enumeration(name) => {
            let name = format_ident!("{name}");
            quote! { #name }
        }
        ReturnType::Boolean if optional && boolean_default.is_none() => quote! { Option<bool> },
        ReturnType::Boolean => quote! { bool },
        ReturnType::UnsignedLong => quote! { u32 },
        ReturnType::Long => quote! { i32 },
        ReturnType::Double => quote! { f64 },
        _ => {
            return Err(Error(
                "native operation argument type is not supported yet".into(),
            ));
        }
    })
}

fn property_getter(
    database: &Database<'_>,
    member: &weedle::interface::OperationInterfaceMember<'_>,
) -> Result<PropertyGetter, Error> {
    let [Argument::Single(argument)] = member.args.body.list.as_slice() else {
        return Err(Error("property getters require one argument".into()));
    };
    if argument.optional.is_some() || argument.default.is_some() {
        return Err(Error("property getter arguments must be required".into()));
    }
    match native_type(database, &argument.type_.type_, &mut BTreeSet::new())? {
        ReturnType::UnsignedLong => Ok(PropertyGetter::Indexed),
        ReturnType::String => Ok(PropertyGetter::Named),
        _ => Err(Error(
            "property getters require DOMString or unsigned long".into(),
        )),
    }
}

fn operation_result(
    database: &Database<'_>,
    return_type: &weedle::types::ReturnType<'_>,
) -> Result<(OperationResult, TokenStream), Error> {
    match return_type {
        weedle::types::ReturnType::Undefined(_) => Ok((OperationResult::Undefined, quote! { () })),
        weedle::types::ReturnType::Type(type_) => {
            match native_type(database, type_, &mut BTreeSet::new())? {
                ReturnType::Node
                | ReturnType::NullableNode
                | ReturnType::PlatformObject
                | ReturnType::NodeList => Ok((OperationResult::Object, quote! { Value<'js> })),
                ReturnType::String => {
                    Ok((OperationResult::String, quote! { rquickjs::String<'js> }))
                }
                ReturnType::NullableString => Ok((
                    OperationResult::NullableString,
                    quote! { Option<rquickjs::String<'js>> },
                )),
                ReturnType::Boolean => Ok((OperationResult::Boolean, quote! { bool })),
                ReturnType::UnsignedShort => Ok((OperationResult::UnsignedShort, quote! { u16 })),
                ReturnType::Long => Ok((OperationResult::Long, quote! { i32 })),
                ReturnType::InterfaceSequence => {
                    Ok((OperationResult::Sequence, quote! { Vec<Value<'js>> }))
                }
                ReturnType::StringSequence => {
                    Ok((OperationResult::StringSequence, quote! { Vec<String> }))
                }
                _ => Err(Error(
                    "native operation result type is not supported yet".into(),
                )),
            }
        }
    }
}

fn lower_attribute(
    database: &Database<'_>,
    member: &weedle::interface::AttributeInterfaceMember<'_>,
    implemented: &BTreeMap<String, Method>,
) -> Result<Option<(model::Attribute, TokenStream)>, Error> {
    let getter_name = format!("get_{}", snake_case(member.identifier.0));
    let setter_name = format!("set_{}", snake_case(member.identifier.0));
    let readable = implemented.contains_key(&getter_name);
    let writable = implemented.contains_key(&setter_name);
    if !readable && !writable {
        return Ok(None);
    }
    if !readable || writable != member.readonly.is_none() {
        return Err(Error(format!(
            "{}: implementation does not match attribute mutability",
            member.identifier.0
        )));
    }
    if !implemented[&getter_name].has_self || implemented[&getter_name].parameters != 0 {
        return Err(Error(format!(
            "{}: attribute getter must take only self and ctx",
            member.identifier.0
        )));
    }
    if writable
        && (!implemented[&setter_name].has_self || implemented[&setter_name].parameters != 1)
    {
        return Err(Error(format!(
            "{}: attribute setter must take only self, ctx, and the value",
            member.identifier.0
        )));
    }
    if member.modifier.is_some()
        && !matches!(
            member.modifier,
            Some(weedle::interface::StringifierOrInheritOrStatic::Stringifier(_))
        )
    {
        return Err(Error(
            "native special attributes are not supported yet".into(),
        ));
    }
    validate_attribute_attributes(member.attributes.as_ref())?;
    validate_argument_attributes(member.type_.attributes.as_ref())?;
    let reactions = has_attribute(member.attributes.as_ref(), "CEReactions");
    let legacy_null_to_empty = has_attribute(member.attributes.as_ref(), "LegacyNullToEmptyString")
        || has_attribute(member.type_.attributes.as_ref(), "LegacyNullToEmptyString");
    let type_ = match native_type(database, &member.type_.type_, &mut BTreeSet::new())? {
        // Getter dispatch hands a platform object or its null directly to JS.
        ReturnType::Node
        | ReturnType::NullableNode
        | ReturnType::NodeList
        | ReturnType::NullableDocumentType => ReturnType::PlatformObject,
        type_ => type_,
    };
    if member.modifier.is_some() && !matches!(type_, ReturnType::String) {
        return Err(Error(
            "native attribute stringifiers require DOMString".into(),
        ));
    }
    let result = match &type_ {
        ReturnType::String => quote! { rquickjs::String<'js> },
        ReturnType::NullableString => quote! { Option<rquickjs::String<'js>> },
        ReturnType::Boolean => quote! { bool },
        ReturnType::UnsignedShort => quote! { u16 },
        ReturnType::UnsignedLong => quote! { usize },
        ReturnType::Long => quote! { i32 },
        ReturnType::Double => quote! { f64 },
        ReturnType::PlatformObject => quote! { Value<'js> },
        ReturnType::Enumeration(name) => {
            let name = format_ident!("{name}");
            quote! { #name }
        }
        _ => return Err(Error("native attribute type is not supported yet".into())),
    };
    let getter = format_ident!("{getter_name}");
    let mut signature = quote! { fn #getter(&self, ctx: &Ctx<'js>) -> Result<#result>; };
    let setter = if writable {
        let setter = format_ident!("{setter_name}");
        let parameter = setter_parameter(&type_)?;
        signature = quote! { #signature fn #setter(&self, ctx: &Ctx<'js>, value: #parameter) -> Result<()>; };
        Some(model::Setter::Method {
            rust: setter,
            from_js: None,
        })
    } else {
        None
    };
    let attribute = model::Attribute {
        name: member.identifier.0.into(),
        rust: getter,
        return_type: type_,
        mapping: GetterMapping::Method,
        setter,
        legacy_null_to_empty,
        reactions,
    };
    Ok(Some((attribute, signature)))
}

/// The Rust parameter type for one lowered attribute setter, mirroring
/// `emit`'s setter conversions.
fn setter_parameter(type_: &ReturnType) -> Result<TokenStream, Error> {
    Ok(match type_ {
        ReturnType::String => quote! { rquickjs::String<'js> },
        ReturnType::NullableString => quote! { Option<rquickjs::String<'js>> },
        ReturnType::Boolean => quote! { bool },
        ReturnType::UnsignedLong => quote! { u32 },
        ReturnType::Long => quote! { i32 },
        ReturnType::Double => quote! { f64 },
        ReturnType::Value => quote! { Value<'js> },
        ReturnType::Enumeration(name) => {
            let name = format_ident!("{name}");
            quote! { #name }
        }
        _ => {
            return Err(Error(
                "native attribute setter type is not supported yet".into(),
            ));
        }
    })
}

fn lower_constructor(
    database: &Database<'_>,
    member: &weedle::interface::ConstructorInterfaceMember<'_>,
) -> Result<(model::Constructor, TokenStream), Error> {
    validate_empty_attributes(member.attributes.as_ref())?;
    let (arguments, parameters) = constructor_arguments(database, member)?;
    let method = format_ident!("constructor");
    let constructor = model::Constructor {
        rust: method.clone(),
        arguments,
    };
    let signature =
        quote! { fn #method(ctx: &Ctx<'js>, #(#parameters),*) -> Result<Self> where Self: Sized; };
    Ok((constructor, signature))
}

fn constructor_arguments(
    database: &Database<'_>,
    member: &weedle::interface::ConstructorInterfaceMember<'_>,
) -> Result<(Vec<model::ConstructorArgument>, Vec<TokenStream>), Error> {
    let mut arguments = Vec::new();
    let mut parameters = Vec::new();
    for (index, argument) in member.args.body.list.iter().enumerate() {
        let Argument::Single(argument) = argument else {
            return Err(Error(
                "native variadic constructors are not supported yet".into(),
            ));
        };
        validate_empty_attributes(argument.attributes.as_ref())?;
        validate_empty_attributes(argument.type_.attributes.as_ref())?;
        let type_ = native_type(database, &argument.type_.type_, &mut BTreeSet::new())?;
        let (kind, parameter) = match type_ {
            ReturnType::String => {
                let default = match (&argument.optional, &argument.default) {
                    (None, None) => None,
                    (Some(_), Some(default)) => {
                        let DefaultValue::String(default) = &default.value else {
                            return Err(Error(
                                "native constructor default is not supported yet".into(),
                            ));
                        };
                        Some(default.0.into())
                    }
                    _ => {
                        return Err(Error(
                            "native constructor optionality is not supported yet".into(),
                        ));
                    }
                };
                (
                    ConstructorArgumentKind::String { default },
                    quote! { rquickjs::String<'js> },
                )
            }
            ReturnType::Callback => match (&argument.optional, &argument.default) {
                (None, None) => (
                    ConstructorArgumentKind::Callback,
                    quote! { rquickjs::Function<'js> },
                ),
                _ => {
                    return Err(Error(
                        "native optional callback arguments are not supported yet".into(),
                    ));
                }
            },
            ReturnType::Dictionary(name) => match (&argument.optional, &argument.default) {
                (Some(_), Some(default))
                    if matches!(default.value, DefaultValue::EmptyDictionary(_)) =>
                {
                    let struct_name = format_ident!("{name}");
                    (
                        ConstructorArgumentKind::Dictionary(name),
                        quote! { #struct_name },
                    )
                }
                _ => {
                    return Err(Error(
                        "native constructor dictionaries need a trailing empty-object default"
                            .into(),
                    ));
                }
            },
            _ => {
                return Err(Error(
                    "native constructor argument type is not supported yet".into(),
                ));
            }
        };
        arguments.push(model::ConstructorArgument { kind });
        let name = format_ident!("arg_{index}");
        parameters.push(quote! { #name: #parameter });
    }
    Ok((arguments, parameters))
}

fn native_type(
    database: &Database<'_>,
    type_: &Type<'_>,
    visited: &mut BTreeSet<String>,
) -> Result<ReturnType, Error> {
    match type_ {
        Type::Single(SingleType::NonAny(NonAnyType::DOMString(item))) => {
            Ok(if item.q_mark.is_some() {
                ReturnType::NullableString
            } else {
                ReturnType::String
            })
        }
        Type::Single(SingleType::NonAny(NonAnyType::USVString(item))) if item.q_mark.is_none() => {
            Ok(ReturnType::UsvString)
        }
        Type::Single(SingleType::NonAny(NonAnyType::Boolean(item))) if item.q_mark.is_none() => {
            Ok(ReturnType::Boolean)
        }
        Type::Single(SingleType::NonAny(NonAnyType::Integer(item)))
            if item.q_mark.is_none()
                && matches!(item.type_, IntegerType::Short(item) if item.unsigned.is_some()) =>
        {
            Ok(ReturnType::UnsignedShort)
        }
        Type::Single(SingleType::NonAny(NonAnyType::Integer(item)))
            if item.q_mark.is_none()
                && matches!(item.type_, IntegerType::Long(item) if item.unsigned.is_some()) =>
        {
            Ok(ReturnType::UnsignedLong)
        }
        Type::Single(SingleType::NonAny(NonAnyType::Integer(item)))
            if item.q_mark.is_none()
                && matches!(item.type_, IntegerType::Long(item) if item.unsigned.is_none()) =>
        {
            Ok(ReturnType::Long)
        }
        Type::Single(SingleType::NonAny(NonAnyType::Sequence(item))) if item.q_mark.is_none() => {
            let element = &item.type_.generics.body;
            validate_empty_attributes(element.attributes.as_ref())?;
            match &element.type_ {
                Type::Single(SingleType::NonAny(NonAnyType::DOMString(element)))
                    if element.q_mark.is_none() =>
                {
                    Ok(ReturnType::StringSequence)
                }
                Type::Single(SingleType::NonAny(NonAnyType::Identifier(element)))
                    if element.q_mark.is_none()
                        && matches!(
                            database.definition(element.type_.0),
                            Some(weedle::Definition::Interface(_))
                        ) =>
                {
                    Ok(ReturnType::InterfaceSequence)
                }
                _ => Err(Error(
                    "only sequences of known interfaces or DOMString are supported yet".into(),
                )),
            }
        }
        Type::Single(SingleType::NonAny(NonAnyType::Identifier(item))) => {
            let name = item.type_.0;
            let nullable = item.q_mark.is_some();
            if let Some(interface) = native_interface(database, name, nullable) {
                return Ok(interface);
            }
            if nullable {
                return Err(Error(format!(
                    "nullable native type {name} is not supported yet"
                )));
            }
            if !visited.insert(name.into()) {
                return Err(Error(format!("typedef cycle at {name}")));
            }
            match database.definition(name) {
                Some(weedle::Definition::Typedef(definition)) => {
                    validate_empty_attributes(definition.attributes.as_ref())?;
                    validate_empty_attributes(definition.type_.attributes.as_ref())?;
                    native_type(database, &definition.type_.type_, visited)
                }
                Some(weedle::Definition::Callback(definition)) => {
                    model::Callback::parse(definition)?;
                    Ok(ReturnType::Callback)
                }
                Some(weedle::Definition::Dictionary(_)) => Ok(ReturnType::Dictionary(name.into())),
                Some(weedle::Definition::Enum(_)) => Ok(ReturnType::Enumeration(name.into())),
                _ => Err(Error(format!("native type {name} is not supported yet"))),
            }
        }
        _ => Err(Error(format!(
            "native type is not supported yet: {type_:?}"
        ))),
    }
}

/// Interface types the contract path carries. Node arguments need the
/// `NodeReference` conversion; every other DOM interface is a platform object
/// whose getter or result hands the value to JS directly.
fn native_interface(database: &Database<'_>, name: &str, nullable: bool) -> Option<ReturnType> {
    match (name, nullable) {
        ("Node", false) => Some(ReturnType::Node),
        ("Node", true) => Some(ReturnType::NullableNode),
        ("DocumentType", true) => Some(ReturnType::NullableDocumentType),
        _ if matches!(
            database.definition(name),
            Some(weedle::Definition::Interface(_))
        ) =>
        {
            Some(ReturnType::PlatformObject)
        }
        _ => None,
    }
}

/// Attribute extended attributes the contract path consumes or documents.
fn validate_attribute_attributes(
    attributes: Option<&ExtendedAttributeList<'_>>,
) -> Result<(), Error> {
    if let Some(attributes) = attributes {
        for attribute in &attributes.body.list {
            match attribute {
                ExtendedAttribute::NoArgs(item)
                    if matches!(
                        item.0.0,
                        "SameObject" | "CEReactions" | "LegacyNullToEmptyString"
                    ) => {}
                _ => {
                    return Err(Error(format!(
                        "native attribute semantics are not supported yet: {attribute:?}"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Operation extended attributes the contract path consumes.
fn validate_operation_attributes(
    attributes: Option<&ExtendedAttributeList<'_>>,
) -> Result<(), Error> {
    if let Some(attributes) = attributes {
        for attribute in &attributes.body.list {
            match attribute {
                ExtendedAttribute::NoArgs(item)
                    if matches!(item.0.0, "NewObject" | "CEReactions") => {}
                _ => {
                    return Err(Error(format!(
                        "native operation semantics are not supported yet: {attribute:?}"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Argument extended attributes the contract path consumes.
fn validate_argument_attributes(
    attributes: Option<&ExtendedAttributeList<'_>>,
) -> Result<(), Error> {
    if let Some(attributes) = attributes {
        for attribute in &attributes.body.list {
            match attribute {
                ExtendedAttribute::NoArgs(item) if item.0.0 == "LegacyNullToEmptyString" => {}
                _ => {
                    return Err(Error(format!(
                        "native argument semantics are not supported yet: {attribute:?}"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_empty_attributes(attributes: Option<&ExtendedAttributeList<'_>>) -> Result<(), Error> {
    if attributes.is_some_and(|attributes| !attributes.body.list.is_empty()) {
        Err(Error(format!(
            "native contract semantics are not supported yet: {attributes:?}"
        )))
    } else {
        Ok(())
    }
}

fn validate_interface_attributes(
    attributes: Option<&ExtendedAttributeList<'_>>,
) -> Result<(), Error> {
    let mut exposed = false;
    if let Some(attributes) = attributes {
        for attribute in &attributes.body.list {
            match attribute {
                ExtendedAttribute::WildCard(item) if item.identifier.0 == "Exposed" => {
                    exposed = true;
                }
                ExtendedAttribute::Ident(item) if item.lhs_identifier.0 == "Exposed" => {
                    exposed = item.rhs.0 == "Window";
                }
                ExtendedAttribute::IdentList(item) if item.identifier.0 == "Exposed" => {
                    exposed = item.list.body.list.iter().any(|item| item.0 == "Window");
                }
                ExtendedAttribute::NoArgs(item)
                    if matches!(
                        item.0.0,
                        "Serializable"
                            | "LegacyUnenumerableNamedProperties"
                            | "LegacyOverrideBuiltIns"
                    ) => {}
                _ => {
                    return Err(Error(format!(
                        "native interface semantics are not supported yet: {attribute:?}"
                    )));
                }
            }
        }
    }
    if exposed {
        Ok(())
    } else {
        Err(Error("native interface is not exposed in Window".into()))
    }
}
