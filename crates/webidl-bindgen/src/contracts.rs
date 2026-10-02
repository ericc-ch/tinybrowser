use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use weedle::argument::Argument;
use weedle::attribute::{ExtendedAttribute, ExtendedAttributeList};
use weedle::interface::InterfaceMember;
use weedle::literal::DefaultValue;
use weedle::types::{IntegerType, NonAnyType, SingleType, Type};

use crate::database::Database;
use crate::model::{
    self, ConstructorArgumentKind, CtxMode, GetterMapping, InterfaceKind, PropertyHooks,
    PrototypeParent, ReturnType,
};
use crate::names::snake_case;
use crate::{Binding, Error, Source};

struct Implementation {
    interface: String,
    payload: syn::Ident,
    lifetime: bool,
    methods: BTreeSet<String>,
}

pub(crate) fn compile(idl: &[Source<'_>], rust: &[Source<'_>]) -> Result<Vec<Binding>, Error> {
    let database = Database::parse(idl)?;
    let mut implementations = BTreeMap::new();
    for source in rust {
        let syntax = syn::parse_file(source.text)
            .map_err(|error| Error(format!("{}: invalid Rust: {error}", source.name)))?;
        discover(&syntax.items, source.name, &mut implementations)?;
    }
    implementations
        .into_values()
        .map(|implementation| {
            let interface = lower(&database, &implementation)?;
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
) -> Result<(), Error> {
    for item in items {
        if let syn::Item::Mod(module) = item
            && let Some((_, items)) = &module.content
        {
            discover(items, source, implementations)?;
        }
        let syn::Item::Impl(item) = item else {
            continue;
        };
        let Some((path, _)) = &item.trait_ else {
            continue;
        };
        let segments: Vec<_> = path.segments.iter().collect();
        let [module, interface] = segments.as_slice() else {
            continue;
        };
        if !module.ident.to_string().ends_with("_generated") {
            continue;
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
            || !item.attrs.is_empty()
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
        let mut methods = BTreeSet::new();
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
            if !methods.insert(member.sig.ident.to_string()) {
                return Err(Error(format!(
                    "{source}: duplicate binding method {}",
                    member.sig.ident
                )));
            }
        }
        let implementation = Implementation {
            interface: name.clone(),
            payload: payload.0,
            lifetime: payload.1,
            methods,
        };
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
) -> Result<model::Interface, Error> {
    let declaration = database.interface(&implementation.interface)?;
    validate_interface_attributes(declaration.attributes.as_ref())?;
    let parent = if let Some(parent) = declaration.parent {
        Some(PrototypeParent::Interface(parent.into()))
    } else if declaration.name == "DOMException" {
        // https://webidl.spec.whatwg.org/#js-DOMException-specialness
        Some(PrototypeParent::Intrinsic("Error".into()))
    } else {
        None
    };
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
        kind: InterfaceKind::Complete,
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
    let mut remaining = implementation.methods.clone();
    let mut methods = Vec::new();
    for member in &declaration.members {
        match member {
            InterfaceMember::Const(member) => {
                interface.constants.push(model::Constant::parse(member)?);
            }
            InterfaceMember::Constructor(member)
                if implementation.methods.contains("constructor") =>
            {
                if interface.constructor.is_some() {
                    return Err(Error(
                        "native constructor overloads are not supported yet".into(),
                    ));
                }
                remaining.remove("constructor");
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
                remaining.remove(&attribute.rust.to_string());
                methods.push(signature);
                interface.attributes.push(attribute);
            }
            InterfaceMember::Constructor(_)
            | InterfaceMember::Operation(_)
            | InterfaceMember::Iterable(_)
            | InterfaceMember::AsyncIterable(_)
            | InterfaceMember::Maplike(_)
            | InterfaceMember::Setlike(_)
            | InterfaceMember::Stringifier(_) => {}
        }
    }
    if !remaining.is_empty() {
        return Err(Error(format!(
            "{}: methods do not match a supported IDL contract: {}",
            declaration.name,
            remaining.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let name = format_ident!("{}", declaration.name);
    interface.contract = Some(quote! { pub(super) trait #name<'js> { #(#methods)* } });
    Ok(interface)
}

fn lower_attribute(
    database: &Database<'_>,
    member: &weedle::interface::AttributeInterfaceMember<'_>,
    implemented: &BTreeSet<String>,
) -> Result<Option<(model::Attribute, TokenStream)>, Error> {
    let getter_name = format!("get_{}", snake_case(member.identifier.0));
    let setter_name = format!("set_{}", snake_case(member.identifier.0));
    let readable = implemented.contains(&getter_name);
    let writable = implemented.contains(&setter_name);
    if !readable && !writable {
        return Ok(None);
    }
    if !readable || writable != member.readonly.is_none() {
        return Err(Error(format!(
            "{}: implementation does not match attribute mutability",
            member.identifier.0
        )));
    }
    if member.modifier.is_some() || writable {
        return Err(Error(
            "native writable and special attributes are not supported yet".into(),
        ));
    }
    validate_empty_attributes(member.attributes.as_ref())?;
    validate_empty_attributes(member.type_.attributes.as_ref())?;
    let type_ = native_type(database, &member.type_.type_, &mut BTreeSet::new())?;
    let result = match type_ {
        ReturnType::String => quote! { rquickjs::String<'js> },
        ReturnType::Boolean => quote! { bool },
        ReturnType::UnsignedShort => quote! { u16 },
        _ => return Err(Error("native attribute type is not supported yet".into())),
    };
    let getter = format_ident!("{getter_name}");
    let signature = quote! { fn #getter(&self, ctx: &Ctx<'js>) -> Result<#result>; };
    let attribute = model::Attribute {
        name: member.identifier.0.into(),
        rust: getter,
        return_type: type_,
        mapping: GetterMapping::Method,
        setter: None,
        legacy_null_to_empty: false,
        reactions: false,
    };
    Ok(Some((attribute, signature)))
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
        if !matches!(
            native_type(database, &argument.type_.type_, &mut BTreeSet::new())?,
            ReturnType::String
        ) {
            return Err(Error("native constructor type is not supported yet".into()));
        }
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
        arguments.push(model::ConstructorArgument {
            kind: ConstructorArgumentKind::String { default },
        });
        let name = format_ident!("arg_{index}");
        parameters.push(quote! { #name: rquickjs::String<'js> });
    }
    Ok((arguments, parameters))
}

fn native_type(
    database: &Database<'_>,
    type_: &Type<'_>,
    visited: &mut BTreeSet<String>,
) -> Result<ReturnType, Error> {
    match type_ {
        Type::Single(SingleType::NonAny(NonAnyType::DOMString(item))) if item.q_mark.is_none() => {
            Ok(ReturnType::String)
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
        Type::Single(SingleType::NonAny(NonAnyType::Identifier(item))) if item.q_mark.is_none() => {
            if !visited.insert(item.type_.0.into()) {
                return Err(Error(format!("typedef cycle at {}", item.type_.0)));
            }
            match database.definition(item.type_.0) {
                Some(weedle::Definition::Typedef(definition)) => {
                    validate_empty_attributes(definition.attributes.as_ref())?;
                    validate_empty_attributes(definition.type_.attributes.as_ref())?;
                    native_type(database, &definition.type_.type_, visited)
                }
                _ => Err(Error(format!(
                    "native type {} is not supported yet",
                    item.type_.0
                ))),
            }
        }
        _ => Err(Error(format!(
            "native type is not supported yet: {type_:?}"
        ))),
    }
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
                ExtendedAttribute::NoArgs(item) if item.0.0 == "Serializable" => {}
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
