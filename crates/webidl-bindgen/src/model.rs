//! Reject unsupported semantics while lowering parsed Web IDL into binding data.

use std::collections::{BTreeMap, HashSet};

use proc_macro2::Ident;
use weedle::argument::Argument;
use weedle::attribute::{ExtendedAttribute, ExtendedAttributeList, IdentifierOrString};
use weedle::interface::InterfaceMember;
use weedle::literal::{ConstValue, DefaultValue, IntegerLit};
use weedle::types::{ConstType, IntegerType, NonAnyType, SingleType, Type};
use weedle::{Definition, Definitions, Parse};

use crate::Error;

pub(crate) struct Interface {
    pub(crate) name: String,
    pub(crate) rust: Ident,
    pub(crate) alternate: Option<Ident>,
    pub(crate) alternate_has_lifetime: bool,
    pub(crate) has_lifetime: bool,
    pub(crate) parent: Option<PrototypeParent>,
    pub(crate) constructor: Option<Constructor>,
    pub(crate) attributes: Vec<Attribute>,
    pub(crate) constants: Vec<Constant>,
    pub(crate) kind: InterfaceKind,
    pub(crate) operations: Vec<Operation>,
    pub(crate) dictionaries: Vec<Dictionary>,
    pub(crate) enumerations: Vec<Enumeration>,
}

pub(crate) enum InterfaceKind {
    Complete,
    Partial,
}

pub(crate) enum PrototypeParent {
    Intrinsic(String),
    Interface(String),
}

pub(crate) struct Operation {
    pub(crate) name: String,
    pub(crate) rust: Ident,
    pub(crate) result: OperationResult,
    pub(crate) takes_this: bool,
    pub(crate) arguments: Vec<OperationArgument>,
    pub(crate) reactions: bool,
}

pub(crate) enum OperationResult {
    /// The method returns the platform object or value to hand back.
    Object,
    Undefined,
    RecordSequence,
    /// The method returns `rquickjs::String<'js>`.
    String,
    Boolean,
    UnsignedShort,
}

pub(crate) struct OperationArgument {
    pub(crate) type_: ReturnType,
    pub(crate) optional: bool,
    pub(crate) null_default: bool,
    pub(crate) legacy_null_to_empty: bool,
    pub(crate) boolean_default: Option<bool>,
}

pub(crate) struct Constructor {
    pub(crate) rust: Ident,
    pub(crate) arguments: Vec<ConstructorArgument>,
}

pub(crate) struct ConstructorArgument {
    pub(crate) kind: ConstructorArgumentKind,
}

pub(crate) enum ConstructorArgumentKind {
    String { default: Option<String> },
    Callback,
}

impl ConstructorArgumentKind {
    pub(crate) fn is_required(&self) -> bool {
        match self {
            Self::String { default } => default.is_none(),
            Self::Callback => true,
        }
    }
}

pub(crate) struct Attribute {
    pub(crate) name: String,
    pub(crate) rust: Ident,
    pub(crate) return_type: ReturnType,
    pub(crate) mapping: GetterMapping,
    pub(crate) setter: Option<Ident>,
    pub(crate) legacy_null_to_empty: bool,
    pub(crate) reactions: bool,
}

pub(crate) enum GetterMapping {
    Method,
    Field,
}

pub(crate) enum ReturnType {
    String,
    NullableString,
    UnsignedShort,
    UnsignedLong,
    Boolean,
    UsvString,
    NullableDocumentType,
    NullableNode,
    Node,
    NodeList,
    Callback,
    Dictionary(String),
    Enumeration(String),
    RecordSequence,
    PlatformObject,
    /// Explicit `[RustValue]` implementation mapping; the method converts it.
    Value,
}

pub(crate) struct Callback {
    pub(crate) name: String,
}

pub(crate) struct Enumeration {
    pub(crate) name: String,
    pub(crate) values: Vec<(String, Ident)>,
}

pub(crate) struct Dictionary {
    pub(crate) name: String,
    pub(crate) fields: Vec<DictionaryField>,
}

pub(crate) struct DictionaryField {
    pub(crate) name: String,
    pub(crate) rust: Ident,
    pub(crate) type_: DictionaryFieldType,
}

pub(crate) enum DictionaryFieldType {
    Boolean { default: Option<bool> },
    StringSequence,
}

pub(crate) struct Constant {
    pub(crate) name: String,
    pub(crate) value: u16,
    pub(crate) legacy_name: Option<String>,
}

/// Named types visible to one interface file: callbacks and dictionaries
/// declared alongside it.
pub(crate) struct TypeNames {
    pub(crate) callbacks: HashSet<String>,
    pub(crate) dictionaries: HashSet<String>,
    pub(crate) enumerations: HashSet<String>,
}

impl Interface {
    pub(crate) fn parse(source: &str) -> Result<Self, Error> {
        // weedle::parse asserts on trailing input. Use the fallible parser and
        // reject the unconsumed suffix ourselves so diagnostics stay controlled.
        let (remaining, definitions) = Definitions::parse(source)
            .map_err(|error| Error(format!("invalid Web IDL: {error}")))?;
        if !remaining.trim().is_empty() {
            return Err(Error(format!("invalid Web IDL near {remaining:?}")));
        }
        let parsed = split_definitions(&definitions)?;
        let names = parsed.type_names()?;
        let DefinitionsInFile {
            callbacks: _,
            dictionaries,
            enumerations,
            interface: definition,
        } = parsed;
        let (kind, identifier_token, members, attributes, parent) = match definition {
            Definition::Interface(definition) => (
                InterfaceKind::Complete,
                definition.identifier.0,
                &definition.members.body,
                definition.attributes.as_ref(),
                definition
                    .inheritance
                    .as_ref()
                    .map(|parent| parent.identifier.0),
            ),
            Definition::PartialInterface(definition) => (
                InterfaceKind::Partial,
                definition.identifier.0,
                &definition.members.body,
                definition.attributes.as_ref(),
                None,
            ),
            _ => {
                return Err(Error(
                    "expected a native interface or partial interface".into(),
                ));
            }
        };
        let mut attributes = Attributes::parse(attributes)?;
        if attributes.take("Exposed")?.as_deref() != Some("Window") {
            return Err(Error(
                "native interfaces must declare Exposed=Window".into(),
            ));
        }
        let rust = attributes.rust()?;
        let alternate = attributes
            .take("RustAlternate")?
            .map(|name| {
                syn::parse_str(&name)
                    .map_err(|error| Error(format!("invalid Rust alternate {name:?}: {error}")))
            })
            .transpose()?;
        let alternate_has_lifetime = attributes.flag("RustAlternateLifetime")?;
        if alternate_has_lifetime && alternate.is_none() {
            return Err(Error("RustAlternateLifetime requires RustAlternate".into()));
        }
        let has_lifetime = attributes.flag("RustLifetime")?;
        let intrinsic = attributes.take("RustPrototype")?;
        if intrinsic.as_deref().is_some_and(|name| name != "Error") {
            return Err(Error("unsupported intrinsic prototype".into()));
        }
        if parent.is_some() && intrinsic.is_some() {
            return Err(Error(
                "interface cannot declare two prototype parents".into(),
            ));
        }
        let parent = parent
            .map(|name| identifier(name).map(|name| PrototypeParent::Interface(name.to_owned())))
            .transpose()?
            .or_else(|| intrinsic.map(PrototypeParent::Intrinsic));
        if matches!(&parent, Some(PrototypeParent::Interface(parent)) if parent == identifier_token)
        {
            return Err(Error("an interface cannot inherit from itself".into()));
        }
        attributes.finish()?;
        if matches!(kind, InterfaceKind::Partial) && parent.is_some() {
            return Err(Error(
                "partial interfaces cannot select a prototype parent".into(),
            ));
        }
        let mut result = Self {
            name: identifier(identifier_token)?.into(),
            rust,
            alternate,
            alternate_has_lifetime,
            has_lifetime,
            parent,
            constructor: None,
            attributes: Vec::new(),
            constants: Vec::new(),
            kind,
            operations: Vec::new(),
            dictionaries,
            enumerations,
        };
        result.lower_members(members, &names)?;
        Ok(result)
    }

    fn lower_members(
        &mut self,
        members: &[InterfaceMember<'_>],
        names: &TypeNames,
    ) -> Result<(), Error> {
        let mut taken = HashSet::new();
        for member in members {
            match member {
                InterfaceMember::Constructor(member) => {
                    if matches!(self.kind, InterfaceKind::Partial) {
                        return Err(Error(
                            "partial interfaces cannot declare constructors".into(),
                        ));
                    }
                    if self.constructor.is_some() {
                        return Err(Error("constructor overloads are not supported yet".into()));
                    }
                    self.constructor = Some(Constructor::parse(member, names)?);
                }
                InterfaceMember::Attribute(member) => {
                    check_name(&mut taken, member.identifier.0)?;
                    self.attributes
                        .push(Attribute::parse(member, self.has_lifetime, names)?);
                }
                InterfaceMember::Const(member) => {
                    if matches!(self.kind, InterfaceKind::Partial) {
                        return Err(Error(
                            "partial interface constants are not supported yet".into(),
                        ));
                    }
                    check_name(&mut taken, member.identifier.0)?;
                    self.constants.push(Constant::parse(member)?);
                }
                InterfaceMember::Operation(member) => {
                    let operation = Operation::parse(member, names)?;
                    if !taken.insert(operation.name.clone()) {
                        return Err(Error(format!(
                            "duplicate interface member: {}",
                            operation.name
                        )));
                    }
                    self.operations.push(operation);
                }
                _ => return Err(Error(format!("unsupported interface member: {member:?}"))),
            }
        }
        Ok(())
    }
}

impl Operation {
    fn parse(
        member: &weedle::interface::OperationInterfaceMember<'_>,
        names: &TypeNames,
    ) -> Result<Self, Error> {
        if member.modifier.is_some() || member.special.is_some() {
            return Err(Error("only regular named operations are supported".into()));
        }
        let name = member
            .identifier
            .as_ref()
            .ok_or_else(|| Error("operation needs an identifier".into()))?;
        let mut attributes = Attributes::parse(member.attributes.as_ref())?;
        let rust = attributes.rust()?;
        let takes_this = attributes.flag("RustThis")?;
        let new_object = attributes.flag("NewObject")?;
        let reactions = attributes.flag("CEReactions")?;
        attributes.finish()?;
        if new_object
            && !matches!(
                &member.return_type,
                weedle::types::ReturnType::Type(Type::Single(SingleType::NonAny(NonAnyType::Identifier(value))))
                    if value.q_mark.is_none() && matches!(ReturnType::parse(
                        &Type::Single(SingleType::NonAny(NonAnyType::Identifier(*value))), names
                    )?, ReturnType::Node | ReturnType::PlatformObject)
            )
        {
            return Err(Error(
                "NewObject requires a non-nullable interface result".into(),
            ));
        }
        let result = match &member.return_type {
            weedle::types::ReturnType::Undefined(_) => OperationResult::Undefined,
            weedle::types::ReturnType::Type(type_) => match ReturnType::parse(type_, names)? {
                ReturnType::Node | ReturnType::PlatformObject | ReturnType::NullableString => {
                    OperationResult::Object
                }
                ReturnType::RecordSequence => OperationResult::RecordSequence,
                ReturnType::String => OperationResult::String,
                ReturnType::Boolean => OperationResult::Boolean,
                ReturnType::UnsignedShort => OperationResult::UnsignedShort,
                _ => {
                    return Err(Error(
                            "only Node, undefined, string, and record sequence operation results are supported yet"
                                .into(),
                        ));
                }
            },
        };
        let mut arguments = Vec::new();
        let mut taken = HashSet::new();
        let mut optional_seen = false;
        for argument in &member.args.body.list {
            let Argument::Single(argument) = argument else {
                return Err(Error("variadic operations are not supported yet".into()));
            };
            identifier_spelling(argument.identifier.0)?;
            if !taken.insert(
                argument
                    .identifier
                    .0
                    .strip_prefix('_')
                    .unwrap_or(argument.identifier.0),
            ) {
                return Err(Error("duplicate operation argument".into()));
            }
            let argument = OperationArgument::parse(argument, names)?;
            if optional_seen && !argument.optional {
                return Err(Error(
                    "required arguments after optional ones are not supported yet".into(),
                ));
            }
            optional_seen |= argument.optional;
            arguments.push(argument);
        }
        Ok(Self {
            name: identifier(name.0)?.into(),
            rust,
            result,
            takes_this,
            arguments,
            reactions,
        })
    }
}

impl OperationArgument {
    fn parse(
        argument: &weedle::argument::SingleArgument<'_>,
        names: &TypeNames,
    ) -> Result<Self, Error> {
        let mut argument_attributes = Attributes::parse(argument.attributes.as_ref())?;
        let rust_value = argument_attributes.flag("RustValue")?;
        let mut type_attributes = Attributes::parse(argument.type_.attributes.as_ref())?;
        let legacy_null_to_empty = argument_attributes.flag("LegacyNullToEmptyString")?
            || type_attributes.flag("LegacyNullToEmptyString")?;
        argument_attributes.finish()?;
        type_attributes.finish()?;
        let type_ = if rust_value {
            ReturnType::Value
        } else {
            ReturnType::parse(&argument.type_.type_, names)?
        };
        let optional = argument.optional.is_some();
        let null_default = argument
            .default
            .as_ref()
            .is_some_and(|default| matches!(default.value, DefaultValue::Null(_)));
        let boolean_default = argument
            .default
            .as_ref()
            .and_then(|default| match default.value {
                DefaultValue::Boolean(value) => Some(value.0),
                _ => None,
            });
        match (&type_, optional, &argument.default) {
            (ReturnType::Boolean, true, Some(_)) if boolean_default.is_some() => {}
            (ReturnType::String, true, None) => {}
            (ReturnType::NullableDocumentType, true, Some(_)) if null_default => {}
            (ReturnType::Dictionary(_), true, Some(default))
                if matches!(
                    default.value,
                    weedle::literal::DefaultValue::EmptyDictionary(_)
                ) => {}
            (ReturnType::Value, true, Some(default))
                if rust_value
                    && matches!(
                        default.value,
                        weedle::literal::DefaultValue::EmptyDictionary(_)
                    ) => {}
            (_, true, _) | (_, _, Some(_)) => {
                return Err(Error(
                    "only trailing optional dictionaries with {} defaults are supported yet".into(),
                ));
            }
            _ => {}
        }
        if legacy_null_to_empty && !matches!(type_, ReturnType::String) {
            return Err(Error("LegacyNullToEmptyString requires DOMString".into()));
        }
        if !matches!(
            type_,
            ReturnType::Node
                | ReturnType::NullableNode
                | ReturnType::Callback
                | ReturnType::Dictionary(_)
                | ReturnType::String
                | ReturnType::NullableString
                | ReturnType::Value
                | ReturnType::NullableDocumentType
                | ReturnType::Boolean
                | ReturnType::UnsignedLong
                | ReturnType::Enumeration(_)
        ) {
            return Err(Error(
                    "only Node, callback, dictionary, string, and value operation arguments are supported yet"
                        .into(),
                ));
        }
        Ok(Self {
            type_,
            optional,
            null_default,
            legacy_null_to_empty,
            boolean_default,
        })
    }
}

impl Attribute {
    fn parse(
        member: &weedle::interface::AttributeInterfaceMember<'_>,
        has_lifetime: bool,
        names: &TypeNames,
    ) -> Result<Self, Error> {
        if member.modifier.is_some() {
            return Err(Error("expected a regular attribute".into()));
        }
        let mut attributes = Attributes::parse(member.attributes.as_ref())?;
        let field = attributes.take("RustField")?;
        let (rust, mapping) = if let Some(field) = field {
            let rust = syn::parse_str(&field)
                .map_err(|error| Error(format!("invalid Rust field {field:?}: {error}")))?;
            (rust, GetterMapping::Field)
        } else {
            (attributes.rust()?, GetterMapping::Method)
        };
        let same_object = attributes.flag("SameObject")?;
        let reactions = attributes.flag("CEReactions")?;
        if reactions && member.readonly.is_some() {
            return Err(Error("CEReactions requires a writable attribute".into()));
        }
        let mut type_attributes = Attributes::parse(member.type_.attributes.as_ref())?;
        let legacy_null_to_empty = attributes.flag("LegacyNullToEmptyString")?
            || type_attributes.flag("LegacyNullToEmptyString")?;
        let setter = attributes
            .take("RustSet")?
            .map(|name| {
                syn::parse_str(&name)
                    .map_err(|error| Error(format!("invalid Rust setter {name:?}: {error}")))
            })
            .transpose()?;
        if member.readonly.is_some() == setter.is_some() {
            return Err(Error(
                "writable attributes require RustSet; readonly attributes forbid it".into(),
            ));
        }
        attributes.finish()?;
        type_attributes.finish()?;
        let return_type = ReturnType::parse(&member.type_.type_, names)?;
        if legacy_null_to_empty && !matches!(return_type, ReturnType::String) {
            return Err(Error("LegacyNullToEmptyString requires DOMString".into()));
        }
        if setter.is_some()
            && (!matches!(mapping, GetterMapping::Method)
                || !matches!(return_type, ReturnType::String | ReturnType::NullableString))
        {
            return Err(Error(
                "only method-mapped string setters are supported yet".into(),
            ));
        }
        match (&mapping, &return_type, same_object, has_lifetime) {
            (GetterMapping::Field, ReturnType::Node | ReturnType::NodeList, true, true)
            | (GetterMapping::Field, ReturnType::String, false, true)
            | (
                GetterMapping::Method,
                ReturnType::String
                | ReturnType::UsvString
                | ReturnType::Boolean
                | ReturnType::PlatformObject
                | ReturnType::NullableString
                | ReturnType::UnsignedShort
                | ReturnType::UnsignedLong
                | ReturnType::NullableNode,
                false,
                _,
            )
            | (GetterMapping::Method, ReturnType::NodeList, true, _) => {}
            _ => {
                return Err(Error(
                    "unsupported getter mapping or SameObject declaration".into(),
                ));
            }
        }
        Ok(Self {
            name: identifier(member.identifier.0)?.into(),
            rust,
            return_type,
            mapping,
            setter,
            legacy_null_to_empty,
            reactions,
        })
    }
}

impl Constructor {
    fn parse(
        member: &weedle::interface::ConstructorInterfaceMember<'_>,
        names: &TypeNames,
    ) -> Result<Self, Error> {
        let mut attributes = Attributes::parse(member.attributes.as_ref())?;
        let rust = attributes.rust()?;
        attributes.finish()?;
        let mut arguments = Vec::new();
        let mut optional_seen = false;
        for argument in &member.args.body.list {
            let Argument::Single(argument) = argument else {
                return Err(Error("variadic constructors are not supported yet".into()));
            };
            identifier_spelling(argument.identifier.0)?;
            Attributes::parse(argument.attributes.as_ref())?.finish()?;
            Attributes::parse(argument.type_.attributes.as_ref())?.finish()?;
            let kind = match ReturnType::parse(&argument.type_.type_, names)? {
                ReturnType::String => {
                    let default = match (&argument.optional, &argument.default) {
                        (Some(_), Some(default)) => {
                            let DefaultValue::String(value) = &default.value else {
                                return Err(Error("expected a DOMString default".into()));
                            };
                            optional_seen = true;
                            Some(value.0.into())
                        }
                        (None, None) if !optional_seen => None,
                        _ => {
                            return Err(Error(
                                "expected trailing optional arguments with defaults".into(),
                            ));
                        }
                    };
                    ConstructorArgumentKind::String { default }
                }
                ReturnType::Callback => match (&argument.optional, &argument.default) {
                    (None, None) if !optional_seen => ConstructorArgumentKind::Callback,
                    _ => {
                        return Err(Error(
                            "optional callback arguments are not supported yet".into(),
                        ));
                    }
                },
                _ => {
                    return Err(Error(
                        "only DOMString and callback constructor arguments are supported yet"
                            .into(),
                    ));
                }
            };
            arguments.push(ConstructorArgument { kind });
        }
        Ok(Self { rust, arguments })
    }
}

impl ReturnType {
    fn parse(type_: &Type<'_>, names: &TypeNames) -> Result<Self, Error> {
        match type_ {
            Type::Union(value) if value.q_mark.is_none() && value.type_.body.list.len() == 2 => {
                // https://webidl.spec.whatwg.org/#es-union
                // TrustedHTML has no implementation in this runtime yet, so
                // no input implements that interface: the DOMString arm wins.
                let mut string = false;
                let mut trusted_html = false;
                for member in &value.type_.body.list {
                    let weedle::types::UnionMemberType::Single(member) = member else {
                        return Err(Error("nested unions are not supported yet".into()));
                    };
                    Attributes::parse(member.attributes.as_ref())?.finish()?;
                    match &member.type_ {
                        NonAnyType::DOMString(value) if value.q_mark.is_none() => string = true,
                        NonAnyType::Identifier(value)
                            if value.q_mark.is_none() && value.type_.0 == "TrustedHTML" =>
                        {
                            trusted_html = true;
                        }
                        _ => return Err(Error("unsupported string union member".into())),
                    }
                }
                if string && trusted_html {
                    Ok(Self::String)
                } else {
                    Err(Error("expected TrustedHTML or DOMString".into()))
                }
            }
            Type::Single(SingleType::NonAny(NonAnyType::USVString(value)))
                if value.q_mark.is_none() =>
            {
                Ok(Self::UsvString)
            }
            Type::Single(SingleType::NonAny(NonAnyType::Boolean(value)))
                if value.q_mark.is_none() =>
            {
                Ok(Self::Boolean)
            }
            Type::Single(SingleType::NonAny(NonAnyType::DOMString(value))) => {
                Ok(if value.q_mark.is_some() {
                    Self::NullableString
                } else {
                    Self::String
                })
            }
            Type::Single(SingleType::NonAny(NonAnyType::Integer(value)))
                if value.q_mark.is_none() && is_unsigned_short(value.type_) =>
            {
                Ok(Self::UnsignedShort)
            }
            Type::Single(SingleType::NonAny(NonAnyType::Integer(value)))
                if value.q_mark.is_none()
                    && matches!(value.type_, IntegerType::Long(value) if value.unsigned.is_some()) =>
            {
                Ok(Self::UnsignedLong)
            }
            Type::Single(SingleType::NonAny(NonAnyType::Sequence(value))) => {
                if value.q_mark.is_some() {
                    return Err(Error("nullable sequences are not supported yet".into()));
                }
                match value.type_.generics.body.as_ref() {
                    Type::Single(SingleType::NonAny(NonAnyType::Identifier(element)))
                        if element.type_.0 == "MutationRecord" && element.q_mark.is_none() =>
                    {
                        Ok(Self::RecordSequence)
                    }
                    _ => Err(Error(
                        "only mutation record sequences are supported yet".into(),
                    )),
                }
            }
            Type::Single(SingleType::NonAny(NonAnyType::Identifier(value))) => {
                match value.type_.0 {
                    "Node" => Ok(if value.q_mark.is_some() {
                        Self::NullableNode
                    } else {
                        Self::Node
                    }),
                    "NodeList" if value.q_mark.is_none() => Ok(Self::NodeList),
                    "DocumentType" if value.q_mark.is_some() => Ok(Self::NullableDocumentType),
                    name if value.q_mark.is_none() && names.callbacks.contains(name) => {
                        Ok(Self::Callback)
                    }
                    name if value.q_mark.is_none() && names.dictionaries.contains(name) => {
                        Ok(Self::Dictionary(name.into()))
                    }
                    name if value.q_mark.is_none() && names.enumerations.contains(name) => {
                        Ok(Self::Enumeration(name.into()))
                    }
                    "Document" | "XMLDocument" | "DocumentType" | "Element" => {
                        Ok(Self::PlatformObject)
                    }
                    _ => Err(Error(format!("unsupported interface type: {value:?}"))),
                }
            }
            _ => Err(Error(format!("unsupported binding type: {type_:?}"))),
        }
    }
}

impl Callback {
    fn parse(definition: &weedle::CallbackDefinition<'_>) -> Result<Self, Error> {
        Attributes::parse(definition.attributes.as_ref())?.finish()?;
        // https://webidl.spec.whatwg.org/#idl-callback-functions
        if !matches!(
            definition.return_type,
            weedle::types::ReturnType::Undefined(_)
        ) {
            return Err(Error(
                "only undefined callback results are supported yet".into(),
            ));
        }
        // The parameter list documents the call contract the hand-written
        // delivery code implements; conversion never runs on it, so only the
        // shape is checked here and WPT covers the behavior.
        for argument in &definition.arguments.body.list {
            let Argument::Single(argument) = argument else {
                return Err(Error("variadic callbacks are not supported yet".into()));
            };
            identifier_spelling(argument.identifier.0)?;
            if argument.optional.is_some() || argument.default.is_some() {
                return Err(Error(
                    "optional callback parameters are not supported yet".into(),
                ));
            }
        }
        Ok(Self {
            name: identifier(definition.identifier.0)?.into(),
        })
    }
}

impl Dictionary {
    fn parse(definition: &weedle::DictionaryDefinition<'_>) -> Result<Self, Error> {
        Attributes::parse(definition.attributes.as_ref())?.finish()?;
        if definition.inheritance.is_some() {
            return Err(Error("dictionary inheritance is not supported yet".into()));
        }
        let mut fields = Vec::new();
        let mut taken = HashSet::new();
        for member in &definition.members.body {
            if member.required.is_some() {
                return Err(Error(
                    "required dictionary fields are not supported yet".into(),
                ));
            }
            let mut attributes = Attributes::parse(member.attributes.as_ref())?;
            let rust = attributes.rust()?;
            attributes.finish()?;
            if !taken.insert(member.identifier.0) {
                return Err(Error(format!(
                    "duplicate dictionary field: {}",
                    member.identifier.0
                )));
            }
            let type_ = match &member.type_ {
                Type::Single(SingleType::NonAny(NonAnyType::Boolean(value))) => {
                    if value.q_mark.is_some() {
                        return Err(Error(
                            "nullable dictionary fields are not supported yet".into(),
                        ));
                    }
                    let default = match &member.default {
                        None => None,
                        Some(default) => {
                            let DefaultValue::Boolean(value) = &default.value else {
                                return Err(Error(
                                    "only boolean dictionary defaults are supported yet".into(),
                                ));
                            };
                            Some(value.0)
                        }
                    };
                    DictionaryFieldType::Boolean { default }
                }
                Type::Single(SingleType::NonAny(NonAnyType::Sequence(value))) => {
                    if value.q_mark.is_some() {
                        return Err(Error(
                            "nullable dictionary fields are not supported yet".into(),
                        ));
                    }
                    match value.type_.generics.body.as_ref() {
                        Type::Single(SingleType::NonAny(NonAnyType::DOMString(element)))
                            if element.q_mark.is_none() => {}
                        _ => {
                            return Err(Error(
                                "only string sequence dictionary fields are supported yet".into(),
                            ));
                        }
                    }
                    if member.default.is_some() {
                        return Err(Error(
                            "sequence dictionary defaults are not supported yet".into(),
                        ));
                    }
                    DictionaryFieldType::StringSequence
                }
                _ => {
                    return Err(Error(format!(
                        "unsupported dictionary field type: {:?}",
                        member.type_
                    )));
                }
            };
            fields.push(DictionaryField {
                name: identifier(member.identifier.0)?.into(),
                rust,
                type_,
            });
        }
        // https://webidl.spec.whatwg.org/#js-dictionary
        fields.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(Self {
            name: identifier(definition.identifier.0)?.into(),
            fields,
        })
    }
}

impl Constant {
    fn parse(member: &weedle::interface::ConstMember<'_>) -> Result<Self, Error> {
        let ConstType::Integer(type_) = member.const_type else {
            return Err(Error("expected an unsigned short constant".into()));
        };
        if type_.q_mark.is_some() || !is_unsigned_short(type_.type_) {
            return Err(Error("expected an unsigned short constant".into()));
        }
        let ConstValue::Integer(IntegerLit::Dec(value)) = member.const_value else {
            return Err(Error("expected a decimal constant".into()));
        };
        let value = value
            .0
            .parse()
            .map_err(|error| Error(format!("invalid constant: {error}")))?;
        let mut attributes = Attributes::parse(member.attributes.as_ref())?;
        let legacy_name = attributes.take("RustLegacyName")?;
        attributes.finish()?;
        Ok(Self {
            name: identifier(member.identifier.0)?.into(),
            value,
            legacy_name,
        })
    }
}

fn is_unsigned_short(type_: IntegerType) -> bool {
    matches!(type_, IntegerType::Short(value) if value.unsigned.is_some())
}

/// One interface file is one native interface plus the callbacks and
/// dictionaries its members name.
struct DefinitionsInFile<'a, 'b> {
    callbacks: Vec<Callback>,
    dictionaries: Vec<Dictionary>,
    enumerations: Vec<Enumeration>,
    interface: &'a Definition<'b>,
}

impl DefinitionsInFile<'_, '_> {
    fn type_names(&self) -> Result<TypeNames, Error> {
        // https://webidl.spec.whatwg.org/#idl-names
        let mut taken = HashSet::new();
        let names = TypeNames {
            callbacks: self
                .callbacks
                .iter()
                .map(|callback| callback.name.clone())
                .collect(),
            dictionaries: self
                .dictionaries
                .iter()
                .map(|dictionary| dictionary.name.clone())
                .collect(),
            enumerations: self
                .enumerations
                .iter()
                .map(|enumeration| enumeration.name.clone())
                .collect(),
        };
        let interface = match self.interface {
            Definition::Interface(interface) => interface.identifier.0,
            Definition::PartialInterface(interface) => interface.identifier.0,
            _ => return Err(Error("expected a native interface".into())),
        };
        for name in names
            .callbacks
            .iter()
            .chain(&names.dictionaries)
            .chain(&names.enumerations)
            .map(String::as_str)
            .chain(std::iter::once(identifier(interface)?))
        {
            if !taken.insert(name) {
                return Err(Error(format!("duplicate named definition: {name}")));
            }
        }
        Ok(names)
    }
}

fn split_definitions<'a, 'b>(
    definitions: &'a Definitions<'b>,
) -> Result<DefinitionsInFile<'a, 'b>, Error> {
    let mut callbacks = Vec::new();
    let mut dictionaries = Vec::new();
    let mut enumerations = Vec::new();
    let mut interface = None;
    for definition in definitions.as_slice() {
        match definition {
            Definition::Enum(definition) => {
                Attributes::parse(definition.attributes.as_ref())?.finish()?;
                let name = identifier(definition.identifier.0)?.to_owned();
                if enumerations
                    .iter()
                    .any(|existing: &Enumeration| existing.name == name)
                {
                    return Err(Error(format!("duplicate enumeration: {name}")));
                }
                let mut values = Vec::new();
                let mut taken = HashSet::new();
                for value in &definition.values.body.list {
                    let text = value.value.0;
                    let mut variant = String::new();
                    for word in text
                        .split(|char: char| !char.is_ascii_alphanumeric())
                        .filter(|word| !word.is_empty())
                    {
                        let mut chars = word.chars();
                        if let Some(first) = chars.next() {
                            variant.push(first.to_ascii_uppercase());
                            variant.extend(chars);
                        }
                    }
                    if variant.is_empty() {
                        variant.push_str("Empty");
                    }
                    if !taken.insert(variant.clone()) {
                        return Err(Error(format!(
                            "duplicate Rust enumeration variant: {variant}"
                        )));
                    }
                    let variant = syn::parse_str(&variant)
                        .map_err(|error| Error(format!("invalid enumeration variant: {error}")))?;
                    values.push((text.into(), variant));
                }
                enumerations.push(Enumeration { name, values });
            }
            Definition::Callback(definition) => {
                let callback = Callback::parse(definition)?;
                if callbacks
                    .iter()
                    .any(|existing: &Callback| existing.name == callback.name)
                {
                    return Err(Error(format!("duplicate callback: {}", callback.name)));
                }
                callbacks.push(callback);
            }
            Definition::Dictionary(definition) => {
                let dictionary = Dictionary::parse(definition)?;
                if dictionaries
                    .iter()
                    .any(|existing: &Dictionary| existing.name == dictionary.name)
                {
                    return Err(Error(format!("duplicate dictionary: {}", dictionary.name)));
                }
                dictionaries.push(dictionary);
            }
            Definition::Interface(_) | Definition::PartialInterface(_) => {
                if interface.is_some() {
                    return Err(Error("expected exactly one native interface".into()));
                }
                interface = Some(definition);
            }
            _ => {
                return Err(Error(
                    "only native interfaces, dictionaries, and callbacks are supported yet".into(),
                ));
            }
        }
    }
    let Some(interface) = interface else {
        return Err(Error("expected exactly one native interface".into()));
    };
    Ok(DefinitionsInFile {
        callbacks,
        dictionaries,
        enumerations,
        interface,
    })
}

fn check_name(names: &mut HashSet<String>, name: &str) -> Result<(), Error> {
    let name = identifier(name)?;
    if names.insert(name.into()) {
        Ok(())
    } else {
        Err(Error(format!("duplicate interface member: {name}")))
    }
}

fn identifier(token: &str) -> Result<&str, Error> {
    // https://webidl.spec.whatwg.org/#idl-names
    identifier_spelling(token)?;
    let name = token.strip_prefix('_').unwrap_or(token);
    if matches!(name, "constructor" | "toString" | "toJSON") || name.starts_with('_') {
        Err(Error(format!("reserved identifier: {token}")))
    } else {
        Ok(name)
    }
}

fn identifier_spelling(token: &str) -> Result<(), Error> {
    // https://webidl.spec.whatwg.org/#prod-identifier
    let spelling = token
        .strip_prefix('_')
        .or_else(|| token.strip_prefix('-'))
        .unwrap_or(token);
    if spelling
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && spelling
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(Error(format!("invalid identifier: {token}")))
    }
}

struct Attributes(BTreeMap<String, Option<String>>);

impl Attributes {
    fn parse(list: Option<&ExtendedAttributeList<'_>>) -> Result<Self, Error> {
        let mut result = BTreeMap::new();
        if let Some(list) = list {
            for attribute in &list.body.list {
                let (key, value) = match attribute {
                    ExtendedAttribute::Ident(attribute) => {
                        let value = match attribute.rhs {
                            IdentifierOrString::Identifier(value) => value.0,
                            IdentifierOrString::String(value) => value.0,
                        };
                        (attribute.lhs_identifier.0, Some(value))
                    }
                    ExtendedAttribute::NoArgs(attribute) => (attribute.0.0, None),
                    _ => {
                        return Err(Error(format!(
                            "unsupported extended attribute: {attribute:?}"
                        )));
                    }
                };
                if result.insert(key.into(), value.map(String::from)).is_some() {
                    return Err(Error(format!("duplicate extended attribute: {key}")));
                }
            }
        }
        Ok(Self(result))
    }

    fn take(&mut self, name: &str) -> Result<Option<String>, Error> {
        match self.0.remove(name) {
            None => Ok(None),
            Some(Some(value)) => Ok(Some(value)),
            Some(None) => Err(Error(format!("{name} requires a value"))),
        }
    }

    fn flag(&mut self, name: &str) -> Result<bool, Error> {
        match self.0.remove(name) {
            None => Ok(false),
            Some(None) => Ok(true),
            Some(Some(_)) => Err(Error(format!("{name} does not take a value"))),
        }
    }

    fn rust(&mut self) -> Result<Ident, Error> {
        let value = self
            .take("Rust")?
            .ok_or_else(|| Error("missing Rust mapping".into()))?;
        syn::parse_str(&value)
            .map_err(|error| Error(format!("invalid Rust mapping {value:?}: {error}")))
    }

    fn finish(self) -> Result<(), Error> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error(format!(
                "unsupported extended attributes: {:?}",
                self.0
            )))
        }
    }
}
