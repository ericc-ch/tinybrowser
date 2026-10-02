//! Reject unsupported semantics while lowering parsed Web IDL into binding data.

use std::collections::{BTreeMap, HashSet};

use proc_macro2::Ident;
use weedle::argument::Argument;
use weedle::attribute::{ExtendedAttribute, ExtendedAttributeList};
use weedle::interface::InterfaceMember;
use weedle::literal::{ConstValue, DefaultValue, IntegerLit};
use weedle::types::{ConstType, IntegerType, NonAnyType, SingleType, Type};
use weedle::{Definition, Definitions, Parse};

use crate::Error;

pub(crate) struct Interface {
    pub(crate) contract: Option<proc_macro2::TokenStream>,
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
    pub(crate) properties: PropertyHooks,
    pub(crate) stringifier: Option<usize>,
    pub(crate) value_iterable: bool,
    pub(crate) indexed_setter: Option<IndexedSetter>,
    /// `[RustInstall="A,B"]`: partial-interface members that belong to a spec
    /// mixin, installed on each named interface's prototype instead of one
    /// interface that shares the payload.
    pub(crate) install_targets: Vec<String>,
    /// `[RustOwnedCtx]`: hand-written getters and setters take an owned `Ctx`
    /// instead of `&Ctx`, so generated attribute dispatch clones it.
    pub(crate) ctx_mode: CtxMode,
}

#[derive(Clone, Copy)]
pub(crate) enum CtxMode {
    /// Attribute getters and setters receive `&Ctx`.
    Borrowed,
    /// Attribute getters and setters receive an owned `Ctx`.
    Owned,
}

impl CtxMode {
    pub(crate) const fn is_owned(self) -> bool {
        matches!(self, Self::Owned)
    }
}

pub(crate) struct IndexedSetter {
    pub(crate) rust: Ident,
    pub(crate) reactions: bool,
}

pub(crate) enum PropertyHooks {
    None,
    JavaScript,
    Indexed,
    IndexedNamed {
        names: Ident,
        unenumerable: bool,
        override_builtins: bool,
    },
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
    pub(crate) getter: Option<PropertyGetter>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PropertyGetter {
    Indexed,
    Named,
}

pub(crate) enum OperationResult {
    /// The method returns the platform object or value to hand back.
    Object,
    Undefined,
    Sequence,
    StringSequence,
    /// The method returns `rquickjs::String<'js>`.
    String,
    Boolean,
    UnsignedShort,
    Long,
}

pub(crate) struct OperationArgument {
    pub(crate) type_: ReturnType,
    pub(crate) arity: ArgumentArity,
    pub(crate) null_default: bool,
    pub(crate) legacy_null_to_empty: bool,
    pub(crate) boolean_default: Option<bool>,
    /// `[RustFromJs=PATH]`: convert the raw argument with `PATH::from_js`
    /// instead of a generated conversion. Used for the renderer's exact
    /// code-unit and pristine-string argument types.
    pub(crate) from_js: Option<syn::Path>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgumentArity {
    Required,
    Optional,
    Variadic,
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
    Dictionary(String),
}

impl ConstructorArgumentKind {
    pub(crate) fn is_required(&self) -> bool {
        match self {
            Self::String { default } => default.is_none(),
            Self::Callback => true,
            Self::Dictionary(_) => false,
        }
    }
}

pub(crate) struct Attribute {
    pub(crate) name: String,
    pub(crate) rust: Ident,
    pub(crate) return_type: ReturnType,
    pub(crate) mapping: GetterMapping,
    pub(crate) setter: Option<Setter>,
    pub(crate) legacy_null_to_empty: bool,
    pub(crate) reactions: bool,
}

pub(crate) enum Setter {
    Method {
        rust: Ident,
        from_js: Option<syn::Path>,
    },
    PutForwards {
        target: String,
        /// The attributed type is nullable, so forwarding must no-op when the
        /// getter returns null (`Document.location` on a detached document).
        nullable: bool,
    },
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
    Long,
    Boolean,
    UsvString,
    NullableDocumentType,
    NullableNode,
    Node,
    NodeList,
    Callback,
    Dictionary(String),
    Enumeration(String),
    InterfaceSequence,
    /// `sequence<DOMString>` result, mapped to a JavaScript array of strings.
    StringSequence,
    PlatformObject,
    /// `DOMHighResTimeStamp`, a `double`
    /// (<https://webidl.spec.whatwg.org/#idl-DOMHighResTimeStamp>).
    Double,
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

/// Resolves the inherited prototype and rejects a direct self-inheritance.
fn resolve_parent(
    parent: Option<&str>,
    intrinsic: Option<String>,
    identifier_token: &str,
) -> Result<Option<PrototypeParent>, Error> {
    let parent = parent
        .map(|name| identifier(name).map(|name| PrototypeParent::Interface(name.to_owned())))
        .transpose()?
        .or_else(|| intrinsic.map(PrototypeParent::Intrinsic));
    if matches!(&parent, Some(PrototypeParent::Interface(parent)) if parent == identifier_token) {
        return Err(Error("an interface cannot inherit from itself".into()));
    }
    Ok(parent)
}

/// Interface-level implementation annotations, resolved together.
struct ResolvedAttributes {
    rust: Ident,
    alternate: Option<Ident>,
    alternate_has_lifetime: bool,
    has_lifetime: bool,
    properties: PropertyHooks,
    parent: Option<PrototypeParent>,
    install_targets: Vec<String>,
    ctx_mode: CtxMode,
}

impl Interface {
    /// Resolve the legacy `Rust*` interface annotations.
    fn resolve_attributes(
        mut attributes: Attributes,
        identifier_token: &str,
        parent: Option<&str>,
        kind: &InterfaceKind,
    ) -> Result<ResolvedAttributes, Error> {
        if attributes.take("Exposed")?.as_deref() != Some("Window") {
            return Err(Error(
                "native interfaces must declare Exposed=Window".into(),
            ));
        }
        let rust = attributes.rust()?;
        let alternate = attributes.rust_mapping("RustAlternate")?;
        let alternate_has_lifetime = attributes.flag("RustAlternateLifetime")?;
        if alternate_has_lifetime && alternate.is_none() {
            return Err(Error("RustAlternateLifetime requires RustAlternate".into()));
        }
        let has_lifetime = attributes.flag("RustLifetime")?;
        let properties = attributes.property_hooks()?;
        let intrinsic = attributes.take("RustPrototype")?;
        if intrinsic.as_deref().is_some_and(|name| name != "Error") {
            return Err(Error("unsupported intrinsic prototype".into()));
        }
        if parent.is_some() && intrinsic.is_some() {
            return Err(Error(
                "interface cannot declare two prototype parents".into(),
            ));
        }
        let parent = resolve_parent(parent, intrinsic, identifier_token)?;
        let install_targets = attributes.install_targets()?;
        let ctx_mode = if attributes.flag("RustOwnedCtx")? {
            CtxMode::Owned
        } else {
            CtxMode::Borrowed
        };
        attributes.finish()?;
        if matches!(kind, InterfaceKind::Partial) && parent.is_some() {
            return Err(Error(
                "partial interfaces cannot select a prototype parent".into(),
            ));
        }
        if !install_targets.is_empty() && !matches!(kind, InterfaceKind::Partial) {
            return Err(Error("RustInstall requires a partial interface".into()));
        }
        Ok(ResolvedAttributes {
            rust,
            alternate,
            alternate_has_lifetime,
            has_lifetime,
            properties,
            parent,
            install_targets,
            ctx_mode,
        })
    }

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
        let ResolvedAttributes {
            rust,
            alternate,
            alternate_has_lifetime,
            has_lifetime,
            properties,
            parent,
            install_targets,
            ctx_mode,
        } = Self::resolve_attributes(
            Attributes::parse(attributes)?,
            identifier_token,
            parent,
            &kind,
        )?;
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
            properties,
            stringifier: None,
            value_iterable: false,
            indexed_setter: None,
            install_targets,
            ctx_mode,
            contract: None,
        };

        result.lower_members(members, &names)?;
        result.validate_property_hooks()?;
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
                    if matches!(
                        member.modifier,
                        Some(weedle::interface::StringifierOrInheritOrStatic::Stringifier(_))
                    ) && self.stringifier.replace(self.attributes.len()).is_some()
                    {
                        return Err(Error("only one stringifier is allowed".into()));
                    }
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
                    if matches!(member.special, Some(weedle::interface::Special::Setter(_))) {
                        if self.indexed_setter.is_some() {
                            return Err(Error(
                                "indexed setter overloads are not supported yet".into(),
                            ));
                        }
                        self.indexed_setter = Some(IndexedSetter::parse(member, names)?);
                        continue;
                    }
                    let operation = Operation::parse(member, names, &self.properties)?;
                    if !taken.insert(operation.name.clone()) {
                        return Err(Error(format!(
                            "duplicate interface member: {}",
                            operation.name
                        )));
                    }
                    self.operations.push(operation);
                }
                InterfaceMember::Iterable(member)
                    if !matches!(self.properties, PropertyHooks::None) =>
                {
                    let weedle::interface::IterableInterfaceMember::Single(member) = member else {
                        return Err(Error("only value iterables are supported yet".into()));
                    };
                    let mut attributes = Attributes::parse(member.attributes.as_ref())?;
                    if attributes.take("RustIterable")?.as_deref() != Some("JavaScript")
                        || self.value_iterable
                    {
                        return Err(Error(
                            "value iterables require one explicit JavaScript implementation".into(),
                        ));
                    }
                    attributes.finish()?;
                    self.value_iterable = true;
                    if !matches!(
                        ReturnType::parse(&member.generics.body.type_, names)?,
                        ReturnType::String | ReturnType::Node
                    ) {
                        return Err(Error(
                            "only string and node value iterables are supported yet".into(),
                        ));
                    }
                }
                _ => return Err(Error(format!("unsupported interface member: {member:?}"))),
            }
        }
        Ok(())
    }

    fn validate_property_hooks(&self) -> Result<(), Error> {
        if self.value_iterable
            && !self
                .operations
                .iter()
                .any(|operation| operation.getter == Some(PropertyGetter::Indexed))
        {
            return Err(Error("value iterables require an indexed getter".into()));
        }
        // https://webidl.spec.whatwg.org/#idl-indexed-properties
        // https://webidl.spec.whatwg.org/#idl-named-properties
        for getter in [PropertyGetter::Indexed, PropertyGetter::Named] {
            let count = self
                .operations
                .iter()
                .filter(|operation| operation.getter == Some(getter))
                .count();
            if count > 1 || (count != 0 && matches!(self.kind, InterfaceKind::Partial)) {
                return Err(Error(
                    "property getters must be unique and declared on a complete interface".into(),
                ));
            }
        }
        if matches!(
            self.properties,
            PropertyHooks::Indexed | PropertyHooks::IndexedNamed { .. }
        ) {
            if matches!(self.kind, InterfaceKind::Partial)
                || !self.attributes.iter().any(|attribute| {
                    attribute.name == "length"
                        && matches!(attribute.return_type, ReturnType::UnsignedLong)
                })
            {
                return Err(Error(
                    "indexed hooks require a complete interface with an unsigned long length"
                        .into(),
                ));
            }
            if !self.operations.iter().any(|operation| {
                operation.getter == Some(PropertyGetter::Indexed)
                    && matches!(operation.result, OperationResult::Object)
                    && matches!(operation.arguments[0].type_, ReturnType::UnsignedLong)
                    && !operation.takes_this
                    && !operation.reactions
                    && operation.arguments[0].from_js.is_none()
            }) {
                return Err(Error(
                    "indexed hooks require a value-returning unsigned long getter".into(),
                ));
            }
            if matches!(self.properties, PropertyHooks::IndexedNamed { .. })
                && !self.operations.iter().any(|operation| {
                    operation.getter == Some(PropertyGetter::Named)
                        && matches!(operation.result, OperationResult::Object)
                        && !operation.takes_this
                        && !operation.reactions
                        && operation.arguments[0].from_js.is_none()
                })
            {
                return Err(Error(
                    "named hooks require a value-returning DOMString getter".into(),
                ));
            }
            if self.indexed_setter.is_some()
                && !matches!(self.properties, PropertyHooks::IndexedNamed { .. })
            {
                return Err(Error("indexed setters require named hooks".into()));
            }
        } else if self.indexed_setter.is_some() {
            return Err(Error("indexed setters require indexed hooks".into()));
        }
        Ok(())
    }
}

impl IndexedSetter {
    fn parse(
        member: &weedle::interface::OperationInterfaceMember<'_>,
        names: &TypeNames,
    ) -> Result<Self, Error> {
        // https://webidl.spec.whatwg.org/#idl-indexed-properties: an indexed
        // setter takes an unsigned long index and a value, with no identifier.
        if member.identifier.is_some() {
            return Err(Error("indexed setters must not have an identifier".into()));
        }
        if member.modifier.is_some() {
            return Err(Error("indexed setters must not be static".into()));
        }
        if !matches!(&member.return_type, weedle::types::ReturnType::Undefined(_)) {
            return Err(Error("indexed setters require an undefined result".into()));
        }
        let mut attributes = Attributes::parse(member.attributes.as_ref())?;
        let rust = attributes.rust()?;
        let reactions = attributes.flag("CEReactions")?;
        attributes.finish()?;
        let [Argument::Single(index), Argument::Single(value)] = member.args.body.list.as_slice()
        else {
            return Err(Error("indexed setters require an index and a value".into()));
        };
        if index.optional.is_some() || index.default.is_some() {
            return Err(Error("indexed setter index must be required".into()));
        }
        if !matches!(
            ReturnType::parse(&index.type_.type_, names)?,
            ReturnType::UnsignedLong
        ) {
            return Err(Error("indexed setter index requires unsigned long".into()));
        }
        Attributes::parse(index.attributes.as_ref())?.finish()?;
        Attributes::parse(index.type_.attributes.as_ref())?.finish()?;
        if value.optional.is_some() || value.default.is_some() {
            return Err(Error("indexed setter value must be required".into()));
        }
        let mut value_attributes = Attributes::parse(value.attributes.as_ref())?;
        let mut value_type_attributes = Attributes::parse(value.type_.attributes.as_ref())?;
        let rust_value =
            value_attributes.flag("RustValue")? || value_type_attributes.flag("RustValue")?;
        value_attributes.finish()?;
        value_type_attributes.finish()?;
        if !rust_value {
            return Err(Error("indexed setter values require RustValue".into()));
        }
        // The value type is carried as a JS value; the platform method checks
        // whether it is an option, null, or undefined.
        Ok(Self { rust, reactions })
    }
}

impl Operation {
    fn parse(
        member: &weedle::interface::OperationInterfaceMember<'_>,
        names: &TypeNames,
        properties: &PropertyHooks,
    ) -> Result<Self, Error> {
        if member.modifier.is_some()
            || (member.special.is_some() && matches!(properties, PropertyHooks::None))
        {
            return Err(Error("only regular named operations are supported".into()));
        }
        let getter = property_getter(member, names)?;
        if matches!(properties, PropertyHooks::Indexed) && getter == Some(PropertyGetter::Named) {
            return Err(Error("indexed hooks do not support named getters".into()));
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
        // `[RustValue]` means the platform method returns the JS value directly,
        // so the IDL result type documents the spec shape without resolution.
        let rust_value = attributes.flag("RustValue")?;
        attributes.finish()?;
        if new_object
            && !rust_value
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
        let result = if rust_value {
            OperationResult::Object
        } else {
            match &member.return_type {
                weedle::types::ReturnType::Undefined(_) => OperationResult::Undefined,
                weedle::types::ReturnType::Type(type_) => match ReturnType::parse(type_, names)? {
                    ReturnType::Node
                    | ReturnType::NullableNode
                    | ReturnType::PlatformObject
                    | ReturnType::NullableString
                    | ReturnType::NodeList => OperationResult::Object,
                    ReturnType::InterfaceSequence => OperationResult::Sequence,
                    ReturnType::StringSequence => OperationResult::StringSequence,
                    ReturnType::String => OperationResult::String,
                    ReturnType::Boolean => OperationResult::Boolean,
                    ReturnType::UnsignedShort => OperationResult::UnsignedShort,
                    ReturnType::Long => OperationResult::Long,
                    _ => {
                        return Err(Error(
                            "only Node, undefined, string, interface sequence, boolean, and unsigned short operation results are supported yet"
                                .into(),
                        ));
                    }
                },
            }
        };
        let mut arguments = Vec::new();
        let mut taken = HashSet::new();
        let mut optional_seen = false;
        for (index, argument) in member.args.body.list.iter().enumerate() {
            let name = match argument {
                Argument::Single(argument) => argument.identifier.0,
                Argument::Variadic(argument) => argument.identifier.0,
            };
            identifier_spelling(name)?;
            if !taken.insert(name.strip_prefix('_').unwrap_or(name)) {
                return Err(Error("duplicate operation argument".into()));
            }
            let argument = OperationArgument::parse(argument, names)?;
            if optional_seen && argument.arity == ArgumentArity::Required {
                return Err(Error(
                    "required arguments after optional ones are not supported yet".into(),
                ));
            }
            if argument.arity == ArgumentArity::Variadic && index + 1 != member.args.body.list.len()
            {
                return Err(Error("a variadic argument must be last".into()));
            }
            optional_seen |= argument.arity == ArgumentArity::Optional;
            arguments.push(argument);
        }
        Ok(Self {
            name: identifier(name.0)?.into(),
            rust,
            result,
            takes_this,
            arguments,
            reactions,
            getter,
        })
    }
}

fn property_getter(
    member: &weedle::interface::OperationInterfaceMember<'_>,
    names: &TypeNames,
) -> Result<Option<PropertyGetter>, Error> {
    let Some(special) = &member.special else {
        return Ok(None);
    };
    // https://webidl.spec.whatwg.org/#idl-indexed-properties
    // https://webidl.spec.whatwg.org/#idl-named-properties
    if !matches!(special, weedle::interface::Special::Getter(_)) {
        return Err(Error(
            "only readonly property getters are supported yet".into(),
        ));
    }
    let [Argument::Single(argument)] = member.args.body.list.as_slice() else {
        return Err(Error("property getters require one argument".into()));
    };
    if argument.optional.is_some()
        || argument.default.is_some()
        || matches!(member.return_type, weedle::types::ReturnType::Undefined(_))
    {
        return Err(Error(
            "property getters require DOMString or unsigned long".into(),
        ));
    }
    match ReturnType::parse(&argument.type_.type_, names)? {
        ReturnType::String => Ok(Some(PropertyGetter::Named)),
        ReturnType::UnsignedLong => Ok(Some(PropertyGetter::Indexed)),
        _ => Err(Error(
            "property getters require DOMString or unsigned long".into(),
        )),
    }
}

impl OperationArgument {
    fn parse(argument: &Argument<'_>, names: &TypeNames) -> Result<Self, Error> {
        let (attributes, type_attributes, type_, default, arity) = match argument {
            Argument::Single(argument) => (
                argument.attributes.as_ref(),
                argument.type_.attributes.as_ref(),
                &argument.type_.type_,
                argument.default.as_ref(),
                if argument.optional.is_some() {
                    ArgumentArity::Optional
                } else {
                    ArgumentArity::Required
                },
            ),
            Argument::Variadic(argument) => (
                argument.attributes.as_ref(),
                None,
                &argument.type_,
                None,
                ArgumentArity::Variadic,
            ),
        };
        let mut argument_attributes = Attributes::parse(attributes)?;
        let mut type_attributes = Attributes::parse(type_attributes)?;
        let rust_value =
            argument_attributes.flag("RustValue")? || type_attributes.flag("RustValue")?;
        let from_js = argument_attributes
            .rust_mapping("RustFromJs")?
            .or(type_attributes.rust_mapping("RustFromJs")?);
        let legacy_null_to_empty = argument_attributes.flag("LegacyNullToEmptyString")?
            || type_attributes.flag("LegacyNullToEmptyString")?;
        argument_attributes.finish()?;
        type_attributes.finish()?;
        // `[RustValue]` and `[RustFromJs]` both mean the argument conversion is
        // supplied by the platform method, so the IDL type documents the spec
        // shape without the generator resolving it.
        let type_ = if rust_value || from_js.is_some() {
            ReturnType::Value
        } else {
            ReturnType::parse(type_, names)?
        };
        let optional = arity == ArgumentArity::Optional;
        if arity == ArgumentArity::Variadic
            && !matches!(type_, ReturnType::String | ReturnType::Value)
        {
            return Err(Error(
                "only DOMString and [RustValue] variadics are supported yet".into(),
            ));
        }
        let null_default =
            default.is_some_and(|default| matches!(default.value, DefaultValue::Null(_)));
        let boolean_default = default.and_then(|default| match default.value {
            DefaultValue::Boolean(value) => Some(value.0),
            _ => None,
        });
        validate_optional_default(
            &type_,
            optional,
            default.as_ref().map(|default| &default.value),
            rust_value,
        )?;
        validate_argument_type(&type_, legacy_null_to_empty, from_js.is_some())?;
        Ok(Self {
            type_,
            arity,
            null_default,
            legacy_null_to_empty,
            boolean_default,
            from_js,
        })
    }
}

fn validate_optional_default(
    type_: &ReturnType,
    optional: bool,
    default: Option<&DefaultValue<'_>>,
    rust_value: bool,
) -> Result<(), Error> {
    let null_default = default.is_some_and(|default| matches!(default, DefaultValue::Null(_)));
    let boolean_default = default.and_then(|default| match default {
        DefaultValue::Boolean(value) => Some(value.0),
        _ => None,
    });
    match (type_, optional, default) {
        (ReturnType::Boolean, true, Some(_)) if boolean_default.is_some() => Ok(()),
        (ReturnType::Boolean | ReturnType::String, true, None)
        | (ReturnType::Dictionary(_), true, Some(DefaultValue::EmptyDictionary(_))) => Ok(()),
        (ReturnType::NullableDocumentType, true, Some(_)) if null_default => Ok(()),
        // A `[RustValue]` argument lets the platform method observe the raw
        // argument, including whether it was omitted, so any default is fine.
        (_, true, _) if rust_value => Ok(()),
        (_, true, _) | (_, _, Some(_)) => Err(Error(
            "only trailing optional dictionaries with {} defaults are supported yet".into(),
        )),
        _ => Ok(()),
    }
}

fn validate_argument_type(
    type_: &ReturnType,
    legacy_null_to_empty: bool,
    has_from_js: bool,
) -> Result<(), Error> {
    if legacy_null_to_empty && !matches!(type_, ReturnType::String) {
        return Err(Error("LegacyNullToEmptyString requires DOMString".into()));
    }
    if has_from_js
        || matches!(
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
                | ReturnType::Long
                | ReturnType::Double
                | ReturnType::Enumeration(_)
        )
    {
        Ok(())
    } else {
        Err(Error(
            "only Node, callback, dictionary, string, double, and value operation arguments are supported yet"
                .into(),
        ))
    }
}

impl Attribute {
    fn parse(
        member: &weedle::interface::AttributeInterfaceMember<'_>,
        has_lifetime: bool,
        names: &TypeNames,
    ) -> Result<Self, Error> {
        if member.modifier.is_some()
            && !matches!(
                member.modifier,
                Some(weedle::interface::StringifierOrInheritOrStatic::Stringifier(_))
            )
        {
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
        let rust_value = attributes.flag("RustValue")?;
        // The unforgeable-attribute property shape is installed by the
        // interface's own shim; the generator only needs to accept the
        // annotation (<https://webidl.spec.whatwg.org/#LegacyUnforgeable>).
        let _legacy_unforgeable = attributes.flag("LegacyUnforgeable")?;
        if reactions && member.readonly.is_some() {
            return Err(Error("CEReactions requires a writable attribute".into()));
        }
        let mut type_attributes = Attributes::parse(member.type_.attributes.as_ref())?;
        let legacy_null_to_empty = attributes.flag("LegacyNullToEmptyString")?
            || type_attributes.flag("LegacyNullToEmptyString")?;
        let setter = Setter::parse(
            &mut attributes,
            member.readonly.is_some(),
            rust_value,
            &member.type_.type_,
        )?;
        attributes.finish()?;
        type_attributes.finish()?;
        let return_type = if rust_value {
            ReturnType::Value
        } else {
            ReturnType::parse(&member.type_.type_, names)?
        };
        if member.modifier.is_some() && !matches!(return_type, ReturnType::String) {
            return Err(Error("stringifier attributes require DOMString".into()));
        }
        if legacy_null_to_empty && !matches!(return_type, ReturnType::String) {
            return Err(Error("LegacyNullToEmptyString requires DOMString".into()));
        }
        if matches!(setter, Some(Setter::Method { .. }))
            && (!matches!(mapping, GetterMapping::Method)
                || !matches!(
                    return_type,
                    ReturnType::String
                        | ReturnType::NullableString
                        | ReturnType::Boolean
                        | ReturnType::UnsignedLong
                        | ReturnType::Long
                        | ReturnType::Double
                        | ReturnType::Value
                ))
        {
            return Err(Error(
                "only method-mapped string, boolean, integer, double, and value setters are supported yet".into(),
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
                | ReturnType::Long
                | ReturnType::NullableNode
                | ReturnType::Double
                | ReturnType::Value,
                false,
                _,
            )
            | (GetterMapping::Method, ReturnType::NodeList | ReturnType::Value, true, _) => {}
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

impl Setter {
    fn parse(
        attributes: &mut Attributes,
        readonly: bool,
        rust_value: bool,
        type_: &Type<'_>,
    ) -> Result<Option<Self>, Error> {
        let rust = attributes
            .take("RustSet")?
            .map(|name| {
                syn::parse_str(&name)
                    .map_err(|error| Error(format!("invalid Rust setter {name:?}: {error}")))
            })
            .transpose()?;
        let from_js = attributes.rust_mapping("RustSetFromJs")?;
        let put_forwards = attributes.take("PutForwards")?;
        let put_forwards_nullable = match &put_forwards {
            // https://webidl.spec.whatwg.org/#PutForwards
            Some(_) => match type_ {
                Type::Single(SingleType::NonAny(NonAnyType::Identifier(type_))) => {
                    type_.q_mark.is_some()
                }
                _ => {
                    return Err(Error("PutForwards requires an interface type".into()));
                }
            },
            None => false,
        };
        if readonly == rust.is_some() {
            return Err(Error(
                "writable attributes require RustSet; readonly attributes forbid it".into(),
            ));
        }
        if from_js.is_some() && rust.is_none() {
            return Err(Error("RustSetFromJs requires RustSet".into()));
        }
        match (rust, put_forwards) {
            (Some(rust), None) => Ok(Some(Self::Method { rust, from_js })),
            (None, Some(target)) if readonly && rust_value => Ok(Some(Self::PutForwards {
                target: identifier(&target)?.into(),
                nullable: put_forwards_nullable,
            })),
            (None, Some(_)) => Err(Error(
                "PutForwards requires a readonly platform object".into(),
            )),
            (None, None) => Ok(None),
            (Some(_), Some(_)) => Err(Error("PutForwards forbids RustSet".into())),
        }
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
                ReturnType::Dictionary(name) => match (&argument.optional, &argument.default) {
                    (Some(_), Some(default))
                        if matches!(default.value, DefaultValue::EmptyDictionary(_)) =>
                    {
                        optional_seen = true;
                        ConstructorArgumentKind::Dictionary(name)
                    }
                    _ => {
                        return Err(Error(
                            "only trailing optional dictionaries with {} defaults are supported yet"
                                .into(),
                        ));
                    }
                },
                _ => {
                    return Err(Error(
                        "only DOMString, callback, and dictionary constructor arguments are supported yet"
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
                // TrustedHTML and TrustedType have no implementation in this runtime yet, so
                // no input implements that interface: the DOMString arm wins.
                let mut string = false;
                let mut trusted = false;
                for member in &value.type_.body.list {
                    let weedle::types::UnionMemberType::Single(member) = member else {
                        return Err(Error("nested unions are not supported yet".into()));
                    };
                    Attributes::parse(member.attributes.as_ref())?.finish()?;
                    match &member.type_ {
                        NonAnyType::DOMString(value) if value.q_mark.is_none() => string = true,
                        NonAnyType::Identifier(value)
                            if value.q_mark.is_none()
                                && matches!(value.type_.0, "TrustedHTML" | "TrustedType") =>
                        {
                            trusted = true;
                        }
                        _ => return Err(Error("unsupported string union member".into())),
                    }
                }
                if string && trusted {
                    Ok(Self::String)
                } else {
                    Err(Error("expected a trusted type or DOMString".into()))
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
            // https://webidl.spec.whatwg.org/#idl-floating-point
            Type::Single(SingleType::NonAny(NonAnyType::FloatingPoint(value)))
                if value.q_mark.is_none() =>
            {
                Ok(Self::Double)
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
            Type::Single(SingleType::NonAny(NonAnyType::Integer(value)))
                if value.q_mark.is_none()
                    && matches!(value.type_, IntegerType::Long(value) if value.unsigned.is_none()) =>
            {
                Ok(Self::Long)
            }
            Type::Single(SingleType::NonAny(NonAnyType::Sequence(value))) => {
                if value.q_mark.is_some() {
                    return Err(Error("nullable sequences are not supported yet".into()));
                }
                Attributes::parse(value.type_.generics.body.attributes.as_ref())?.finish()?;
                match &value.type_.generics.body.type_ {
                    Type::Single(SingleType::NonAny(NonAnyType::Identifier(element)))
                        if element.q_mark.is_none() && is_interface_name(element.type_.0) =>
                    {
                        Ok(Self::InterfaceSequence)
                    }
                    Type::Single(SingleType::NonAny(NonAnyType::DOMString(element)))
                        if element.q_mark.is_none() =>
                    {
                        Ok(Self::StringSequence)
                    }
                    _ => Err(Error(
                        "only sequences of known interfaces or DOMString are supported yet".into(),
                    )),
                }
            }
            Type::Single(SingleType::NonAny(NonAnyType::Identifier(value))) => {
                identifier_type(value.type_.0, value.q_mark.is_some(), names)
            }
            _ => Err(Error(format!("unsupported binding type: {type_:?}"))),
        }
    }
}

/// A named IDL type: a callback, dictionary, enumeration, or an interface the
/// generator can carry as a platform object.
fn identifier_type(name: &str, nullable: bool, names: &TypeNames) -> Result<ReturnType, Error> {
    match name {
        "Node" => Ok(if nullable {
            ReturnType::NullableNode
        } else {
            ReturnType::Node
        }),
        "NodeList" if !nullable => Ok(ReturnType::NodeList),
        "DocumentType" if nullable => Ok(ReturnType::NullableDocumentType),
        "DOMHighResTimeStamp" if !nullable => Ok(ReturnType::Double),
        name if !nullable && names.callbacks.contains(name) => Ok(ReturnType::Callback),
        name if !nullable && names.dictionaries.contains(name) => {
            Ok(ReturnType::Dictionary(name.into()))
        }
        name if !nullable && names.enumerations.contains(name) => {
            Ok(ReturnType::Enumeration(name.into()))
        }
        "Document"
        | "XMLDocument"
        | "DocumentType"
        | "Element"
        | "Event"
        | "EventTarget"
        | "DocumentFragment"
        | "Attr"
        | "Text"
        | "Comment"
        | "CDATASection"
        | "ProcessingInstruction"
        | "HTMLCollection"
        | "DOMTokenList"
        | "DOMStringMap" => Ok(ReturnType::PlatformObject),
        _ => Err(Error(format!("unsupported interface type: {name}"))),
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
            Attributes::parse(member.type_.attributes.as_ref())?.finish()?;
            let type_ = match &member.type_.type_ {
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
                    Attributes::parse(value.type_.generics.body.attributes.as_ref())?.finish()?;
                    match &value.type_.generics.body.type_ {
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
    pub(crate) fn parse(member: &weedle::interface::ConstMember<'_>) -> Result<Self, Error> {
        let ConstType::Integer(type_) = member.const_type else {
            return Err(Error("expected an unsigned short constant".into()));
        };
        if type_.q_mark.is_some() || !is_unsigned_short(type_.type_) {
            return Err(Error("expected an unsigned short constant".into()));
        }
        // https://webidl.spec.whatwg.org/#idl-constants
        // weedle lexes `0` as an octal literal, so accept every integer form.
        let value = match member.const_value {
            ConstValue::Integer(IntegerLit::Dec(value)) => parse_const_integer(value.0, 10)?,
            ConstValue::Integer(IntegerLit::Hex(value)) => {
                let digits = value.0.strip_prefix("0x").unwrap_or(value.0);
                parse_const_integer(digits, 16)?
            }
            ConstValue::Integer(IntegerLit::Oct(value)) => parse_const_integer(value.0, 8)?,
            _ => return Err(Error("expected an integer constant".into())),
        };
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

/// Parse a Web IDL integer constant literal
/// (<https://webidl.spec.whatwg.org/#idl-constants>) into its `unsigned short`
/// value.
fn parse_const_integer(digits: &str, radix: u32) -> Result<u16, Error> {
    u16::from_str_radix(digits, radix).map_err(|error| Error(format!("invalid constant: {error}")))
}

/// Interface identifiers the generator can carry as platform objects, so a
/// `sequence<Interface>` element is known to become a JS platform-object array
/// (<https://webidl.spec.whatwg.org/#idl-sequences>).
fn is_interface_name(name: &str) -> bool {
    matches!(
        name,
        "Node"
            | "NodeList"
            | "Document"
            | "XMLDocument"
            | "DocumentType"
            | "Element"
            | "Event"
            | "EventTarget"
            | "MutationRecord"
    )
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
                    let text = value.0;
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
    fn rust_mapping<T: syn::parse::Parse>(&mut self, key: &str) -> Result<Option<T>, Error> {
        self.take(key)?
            .map(|path| {
                syn::parse_str(&path)
                    .map_err(|error| Error(format!("invalid {key} {path:?}: {error}")))
            })
            .transpose()
    }
    fn property_hooks(&mut self) -> Result<PropertyHooks, Error> {
        let names: Option<Ident> = self.rust_mapping("RustSupportedNames")?;
        let unenumerable = self.flag("LegacyUnenumerableNamedProperties")?;
        let override_builtins = self.flag("LegacyOverrideBuiltIns")?;
        let hooks = match self.take("RustPropertyHooks")?.as_deref() {
            None => PropertyHooks::None,
            Some("JavaScript") => PropertyHooks::JavaScript,
            Some("Indexed") => PropertyHooks::Indexed,
            Some("IndexedNamed") => PropertyHooks::IndexedNamed {
                names: names
                    .clone()
                    .ok_or_else(|| Error("named hooks require RustSupportedNames".into()))?,
                unenumerable,
                override_builtins,
            },
            Some(_) => return Err(Error("unsupported property-hook implementation".into())),
        };
        if names.is_some() && !matches!(hooks, PropertyHooks::IndexedNamed { .. }) {
            return Err(Error("RustSupportedNames requires named hooks".into()));
        }
        if unenumerable
            && !matches!(
                hooks,
                PropertyHooks::JavaScript | PropertyHooks::IndexedNamed { .. }
            )
        {
            return Err(Error(
                "named property annotations require property hooks".into(),
            ));
        }
        if override_builtins && !matches!(hooks, PropertyHooks::IndexedNamed { .. }) {
            return Err(Error("LegacyOverrideBuiltIns requires named hooks".into()));
        }
        Ok(hooks)
    }

    fn parse(list: Option<&ExtendedAttributeList<'_>>) -> Result<Self, Error> {
        let mut result = BTreeMap::new();
        if let Some(list) = list {
            for attribute in &list.body.list {
                let (key, value) = match attribute {
                    ExtendedAttribute::Ident(attribute) => {
                        (attribute.lhs_identifier.0, Some(attribute.rhs.0))
                    }
                    ExtendedAttribute::String(attribute) => {
                        (attribute.lhs_identifier.0, Some(attribute.rhs.0))
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

    /// `[RustInstall="A,B"]`: the interfaces a partial (spec mixin) installs on.
    fn install_targets(&mut self) -> Result<Vec<String>, Error> {
        Ok(self
            .take("RustInstall")?
            .map(|list| {
                list.split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default())
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
