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
    pub(crate) has_lifetime: bool,
    pub(crate) parent_intrinsic: Option<String>,
    pub(crate) constructor: Option<Constructor>,
    pub(crate) getters: Vec<Getter>,
    pub(crate) constants: Vec<Constant>,
    pub(crate) kind: InterfaceKind,
    pub(crate) operations: Vec<Operation>,
}

pub(crate) enum InterfaceKind {
    Complete,
    Partial,
}

pub(crate) struct Operation {
    pub(crate) name: String,
    pub(crate) rust: Ident,
    pub(crate) arguments: Vec<ReturnType>,
}

pub(crate) struct Constructor {
    pub(crate) rust: Ident,
    pub(crate) arguments: Vec<StringArgument>,
}

pub(crate) struct StringArgument {
    pub(crate) default: Option<String>,
}

pub(crate) struct Getter {
    pub(crate) name: String,
    pub(crate) rust: Ident,
    pub(crate) return_type: ReturnType,
    pub(crate) mapping: GetterMapping,
}

pub(crate) enum GetterMapping {
    Method,
    Field,
}

pub(crate) enum ReturnType {
    String,
    NullableString,
    UnsignedShort,
    NullableNode,
    Node,
    NodeList,
}

pub(crate) struct Constant {
    pub(crate) name: String,
    pub(crate) value: u16,
    pub(crate) legacy_name: Option<String>,
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
        let [definition] = definitions.as_slice() else {
            return Err(Error("expected exactly one native interface".into()));
        };
        let (kind, identifier_token, members, attributes) = match definition {
            Definition::Interface(definition) => {
                if definition.inheritance.is_some() {
                    return Err(Error("interface inheritance is not supported yet".into()));
                }
                (
                    InterfaceKind::Complete,
                    definition.identifier.0,
                    &definition.members.body,
                    definition.attributes.as_ref(),
                )
            }
            Definition::PartialInterface(definition) => (
                InterfaceKind::Partial,
                definition.identifier.0,
                &definition.members.body,
                definition.attributes.as_ref(),
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
        let has_lifetime = attributes.flag("RustLifetime")?;
        let parent_intrinsic = attributes.take("RustPrototype")?;
        if parent_intrinsic
            .as_deref()
            .is_some_and(|name| name != "Error")
        {
            return Err(Error("unsupported intrinsic prototype".into()));
        }
        attributes.finish()?;
        if matches!(kind, InterfaceKind::Partial) && parent_intrinsic.is_some() {
            return Err(Error(
                "partial interfaces cannot select a prototype parent".into(),
            ));
        }
        let mut result = Self {
            name: identifier(identifier_token)?.into(),
            rust,
            has_lifetime,
            parent_intrinsic,
            constructor: None,
            getters: Vec::new(),
            constants: Vec::new(),
            kind,
            operations: Vec::new(),
        };
        result.lower_members(members)?;
        Ok(result)
    }

    fn lower_members(&mut self, members: &[InterfaceMember<'_>]) -> Result<(), Error> {
        let mut names = HashSet::new();
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
                    self.constructor = Some(Constructor::parse(member)?);
                }
                InterfaceMember::Attribute(member) => {
                    check_name(&mut names, member.identifier.0)?;
                    self.getters.push(Getter::parse(member, self.has_lifetime)?);
                }
                InterfaceMember::Const(member) => {
                    if matches!(self.kind, InterfaceKind::Partial) {
                        return Err(Error(
                            "partial interface constants are not supported yet".into(),
                        ));
                    }
                    check_name(&mut names, member.identifier.0)?;
                    self.constants.push(Constant::parse(member)?);
                }
                InterfaceMember::Operation(member) => {
                    let operation = Operation::parse(member)?;
                    if !names.insert(operation.name.clone()) {
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
    fn parse(member: &weedle::interface::OperationInterfaceMember<'_>) -> Result<Self, Error> {
        if member.modifier.is_some() || member.special.is_some() {
            return Err(Error("only regular named operations are supported".into()));
        }
        let name = member
            .identifier
            .as_ref()
            .ok_or_else(|| Error("operation needs an identifier".into()))?;
        let mut attributes = Attributes::parse(member.attributes.as_ref())?;
        let rust = attributes.rust()?;
        attributes.finish()?;
        let weedle::types::ReturnType::Type(result) = &member.return_type else {
            return Err(Error(
                "only Node operation results are supported yet".into(),
            ));
        };
        if !matches!(ReturnType::parse(result)?, ReturnType::Node) {
            return Err(Error(
                "only Node operation results are supported yet".into(),
            ));
        }
        let mut arguments = Vec::new();
        let mut names = HashSet::new();
        for argument in &member.args.body.list {
            let Argument::Single(argument) = argument else {
                return Err(Error("variadic operations are not supported yet".into()));
            };
            identifier_spelling(argument.identifier.0)?;
            if !names.insert(
                argument
                    .identifier
                    .0
                    .strip_prefix('_')
                    .unwrap_or(argument.identifier.0),
            ) {
                return Err(Error("duplicate operation argument".into()));
            }
            Attributes::parse(argument.attributes.as_ref())?.finish()?;
            Attributes::parse(argument.type_.attributes.as_ref())?.finish()?;
            if argument.optional.is_some() || argument.default.is_some() {
                return Err(Error(
                    "optional operation arguments are not supported yet".into(),
                ));
            }
            let type_ = ReturnType::parse(&argument.type_.type_)?;
            if !matches!(type_, ReturnType::Node | ReturnType::NullableNode) {
                return Err(Error(
                    "only Node operation arguments are supported yet".into(),
                ));
            }
            arguments.push(type_);
        }
        Ok(Self {
            name: identifier(name.0)?.into(),
            rust,
            arguments,
        })
    }
}

impl Getter {
    fn parse(
        member: &weedle::interface::AttributeInterfaceMember<'_>,
        has_lifetime: bool,
    ) -> Result<Self, Error> {
        if member.readonly.is_none() || member.modifier.is_some() {
            return Err(Error("expected a regular readonly attribute".into()));
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
        attributes.finish()?;
        Attributes::parse(member.type_.attributes.as_ref())?.finish()?;
        let return_type = ReturnType::parse(&member.type_.type_)?;
        match (&mapping, &return_type, same_object, has_lifetime) {
            (GetterMapping::Field, ReturnType::Node | ReturnType::NodeList, true, true)
            | (GetterMapping::Field, ReturnType::String, false, true)
            | (
                GetterMapping::Method,
                ReturnType::String
                | ReturnType::NullableString
                | ReturnType::UnsignedShort
                | ReturnType::NullableNode,
                false,
                _,
            ) => {}
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
        })
    }
}

impl Constructor {
    fn parse(member: &weedle::interface::ConstructorInterfaceMember<'_>) -> Result<Self, Error> {
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
            if !matches!(
                ReturnType::parse(&argument.type_.type_)?,
                ReturnType::String
            ) {
                return Err(Error(
                    "only DOMString constructor arguments are supported yet".into(),
                ));
            }
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
            arguments.push(StringArgument { default });
        }
        Ok(Self { rust, arguments })
    }
}

impl ReturnType {
    fn parse(type_: &Type<'_>) -> Result<Self, Error> {
        match type_ {
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
            Type::Single(SingleType::NonAny(NonAnyType::Identifier(value))) => {
                match value.type_.0 {
                    "Node" => Ok(if value.q_mark.is_some() {
                        Self::NullableNode
                    } else {
                        Self::Node
                    }),
                    "NodeList" if value.q_mark.is_none() => Ok(Self::NodeList),
                    _ => Err(Error(format!("unsupported interface result: {value:?}"))),
                }
            }
            _ => Err(Error(format!("unsupported binding type: {type_:?}"))),
        }
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
