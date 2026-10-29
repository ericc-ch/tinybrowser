use std::collections::{BTreeMap, BTreeSet};

use oxc_allocator::Allocator;
use oxc_ast::ast::{self, ClassElement, MethodDefinitionKind};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};
use weedle::attribute::{ExtendedAttribute, ExtendedAttributeList};
use weedle::interface::InterfaceMember;

use crate::contracts::native_type;
use crate::database::Database;
use crate::model::ReturnType;
use crate::{Error, Javascript, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Constructor,
    Method,
    Get,
    Set,
}

pub(crate) fn compile(idl: &[Source<'_>], source: &Source<'_>) -> Result<Javascript, Error> {
    let database = Database::parse(idl)?;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source.text, SourceType::cjs()).parse();
    if let Some(error) = parsed.diagnostics.first() {
        return Err(Error(format!(
            "{}: invalid JavaScript: {error}",
            source.name
        )));
    }
    let mut discovery = Discovery {
        database: &database,
        source: source.text,
        replacements: Vec::new(),
        interfaces: BTreeSet::new(),
        error: None,
    };
    discovery.visit_program(&parsed.program);
    if let Some(error) = discovery.error {
        return Err(Error(format!("{}: {error}", source.name)));
    }
    discovery.replacements.sort_by_key(|(span, _)| span.start);
    let mut output = String::new();
    let mut offset = 0;
    for (span, replacement) in discovery.replacements {
        let start = span.start as usize;
        if start < offset {
            return Err(Error(
                "nested interface installations are not supported".into(),
            ));
        }
        output.push_str(&source.text[offset..start]);
        output.push_str(&replacement);
        offset = span.end as usize;
    }
    output.push_str(&source.text[offset..]);
    Ok(Javascript {
        source: output,
        interfaces: discovery.interfaces.into_iter().collect(),
    })
}

struct Discovery<'db, 'idl, 'source> {
    database: &'db Database<'idl>,
    source: &'source str,
    replacements: Vec<(Span, String)>,
    interfaces: BTreeSet<String>,
    error: Option<Error>,
}

impl<'ast> Visit<'ast> for Discovery<'_, '_, '_> {
    fn visit_call_expression(&mut self, call: &ast::CallExpression<'ast>) {
        if matches!(&call.callee, ast::Expression::Identifier(identifier) if identifier.name == "__tbInstallInterface")
        {
            let result = self.installation(call);
            match result {
                Ok(replacement) => self.replacements.push((call.span, replacement)),
                Err(error) => {
                    let line = self
                        .source
                        .get(..call.span.start as usize)
                        .map_or(0, |prefix| {
                            prefix.bytes().filter(|byte| *byte == b'\n').count() + 1
                        });
                    self.error = Some(Error(format!("line {line}: {error}")));
                }
            }
        }
        walk::walk_call_expression(self, call);
    }
}

impl Discovery<'_, '_, '_> {
    fn installation(&mut self, call: &ast::CallExpression<'_>) -> Result<String, Error> {
        let (class, unimplemented) = match call.arguments.as_slice() {
            [ast::Argument::ClassExpression(class)] => (class, BTreeSet::new()),
            [
                ast::Argument::ClassExpression(class),
                ast::Argument::ArrayExpression(list),
            ] => {
                let mut names = BTreeSet::new();
                for element in &list.elements {
                    let ast::ArrayExpressionElement::StringLiteral(name) = element else {
                        return Err(Error(
                            "unimplemented members must be string literals".into(),
                        ));
                    };
                    names.insert(name.value.as_str().to_owned());
                }
                (class, names)
            }
            _ => {
                return Err(Error(
                    "interface installation requires one named class expression and an optional list of unimplemented members".into(),
                ));
            }
        };
        if call.optional || class.heritage.is_some() || !class.decorators.is_empty() {
            return Err(Error(
                "conditional or inherited JS class installation is not supported yet".into(),
            ));
        }
        let name = class
            .id
            .as_ref()
            .ok_or_else(|| Error("interface class must be named".into()))?
            .name
            .as_str();
        if !self.interfaces.insert(name.into()) {
            return Err(Error(format!("duplicate JS implementation of {name}")));
        }
        let declaration = self.database.interface(name)?;
        let interface_attributes = attributes(
            declaration.attributes.as_ref(),
            &["Exposed", "SecureContext"],
        )?;
        if interface_attributes.contains(&"SecureContext") {
            return Err(Error(
                "a secure-only JS interface is not supported yet: the installer does not own global registration".into(),
            ));
        }
        let exposed = crate::database::exposed_in(declaration.attributes.as_ref(), "Window");
        if !exposed || declaration.parent.is_some() {
            return Err(Error(
                "JS interface exposure or inheritance is not supported yet".into(),
            ));
        }
        let (constructor, members) = lower_members(
            self.database,
            &declaration,
            implementation_methods(class)?,
            &unimplemented,
        )?;
        let constructor = constructor.unwrap_or_else(|| "null".into());
        let implementation = class.span.source_text(self.source);
        Ok(format!(
            "__tbInstallInterface({implementation}, {name:?}, {constructor}, [{}])",
            members.join(",")
        ))
    }
}

fn implementation_methods(
    class: &ast::Class<'_>,
) -> Result<BTreeMap<(String, Kind), usize>, Error> {
    let mut implemented = BTreeMap::new();
    for member in &class.body.body {
        let ClassElement::MethodDefinition(method) = member else {
            return Err(Error(
                "JS interface classes contain algorithm methods only".into(),
            ));
        };
        if method.computed
            || method.r#static
            || method.value.r#async
            || method.value.generator
            || !method.decorators.is_empty()
        {
            return Err(Error(
                "JS special algorithm methods are not supported yet".into(),
            ));
        }
        let key = method
            .key
            .static_name()
            .ok_or_else(|| Error("JS interface member must have an IDL name".into()))?
            .into_owned();
        let kind = match method.kind {
            MethodDefinitionKind::Constructor => Kind::Constructor,
            MethodDefinitionKind::Method => Kind::Method,
            MethodDefinitionKind::Get => Kind::Get,
            MethodDefinitionKind::Set => Kind::Set,
        };
        if method.value.params.rest.is_some()
            || method.value.params.items.iter().any(|parameter| {
                !matches!(parameter.pattern, ast::BindingPattern::BindingIdentifier(_))
            })
        {
            return Err(Error("JS algorithm parameters must be plain names; IDL supplies defaults and conversions".into()));
        }
        if implemented
            .insert((key.clone(), kind), method.value.params.items.len())
            .is_some()
        {
            return Err(Error(format!("duplicate JS member {key}")));
        }
    }
    Ok(implemented)
}

fn lower_members(
    database: &Database<'_>,
    declaration: &crate::database::Interface<'_>,
    mut implemented: BTreeMap<(String, Kind), usize>,
    unimplemented: &BTreeSet<String>,
) -> Result<(Option<String>, Vec<String>), Error> {
    let provided: BTreeSet<_> = implemented.keys().cloned().collect();
    let mut constructor = None;
    let mut members = Vec::new();
    let mut missing = Vec::new();
    for resolved in &declaration.members {
        let exposure = resolved.exposure(declaration);
        match &resolved.declaration {
            InterfaceMember::Constructor(member)
                if provided.contains(&("constructor".into(), Kind::Constructor)) =>
            {
                if constructor.is_some() {
                    return Err(Error(
                        "JS constructor overloads are not supported yet".into(),
                    ));
                }
                constructor = Some(lower_constructor_member(
                    database,
                    member,
                    resolved,
                    &mut implemented,
                )?);
            }
            InterfaceMember::Attribute(member)
                if provided.contains(&(member.identifier.0.into(), Kind::Get))
                    || provided.contains(&(member.identifier.0.into(), Kind::Set)) =>
            {
                lower_attribute_member(
                    database,
                    member,
                    resolved,
                    exposure,
                    &mut implemented,
                    &mut members,
                )?;
            }
            InterfaceMember::Operation(member)
                if member
                    .identifier
                    .is_some_and(|name| provided.contains(&(name.0.into(), Kind::Method))) =>
            {
                lower_operation_member(
                    database,
                    member,
                    resolved,
                    exposure,
                    &mut implemented,
                    &mut members,
                )?;
            }
            InterfaceMember::Constructor(member) => {
                missing.push(format!("constructor({})", member.args.body.list.len()));
            }
            InterfaceMember::Attribute(member) => missing.push(member.identifier.0.to_owned()),
            InterfaceMember::Operation(member) => missing.push(
                member
                    .identifier
                    .map_or_else(|| "operation".to_owned(), |name| name.0.to_owned()),
            ),
            _ => {}
        }
    }
    if let Some(name) = unimplemented
        .iter()
        .find(|name| !declaration.members.iter().any(|member| matches!(
            &member.declaration,
            InterfaceMember::Attribute(attribute) if attribute.identifier.0 == name.as_str()
        ) || matches!(
            &member.declaration,
            InterfaceMember::Operation(operation) if operation.identifier.map(|id| id.0) == Some(name.as_str())
        )))
    {
        return Err(Error(format!(
            "{0}: unimplemented member {name:?} is not declared by the IDL",
            declaration.name
        )));
    }
    let missing: Vec<String> = missing
        .into_iter()
        .filter(|name| !unimplemented.contains(name))
        .collect();
    if !missing.is_empty() {
        return Err(Error(format!(
            "{0}: JS implementation does not cover IDL members {missing:?}; list them in the installation's unimplemented array",
            declaration.name
        )));
    }
    if !implemented.is_empty() {
        let name = declaration.name;
        return Err(Error(format!(
            "{name}: JS members do not match supported IDL contracts: {:?}",
            implemented.keys().collect::<Vec<_>>()
        )));
    }
    Ok((constructor, members))
}

fn lower_constructor_member(
    database: &Database<'_>,
    member: &weedle::interface::ConstructorInterfaceMember<'_>,
    resolved: &crate::database::Member<'_>,
    implemented: &mut BTreeMap<(String, Kind), usize>,
) -> Result<String, Error> {
    resolved.validate_scopes()?;
    attributes(member.attributes.as_ref(), &[])?;
    let (required, conversion) = arguments(database, &member.args.body.list)?;
    signature(
        implemented,
        "constructor",
        Kind::Constructor,
        member.args.body.list.len(),
    )?;
    Ok(format!("[{required}, args => {conversion}]"))
}

fn lower_attribute_member(
    database: &Database<'_>,
    member: &weedle::interface::AttributeInterfaceMember<'_>,
    resolved: &crate::database::Member<'_>,
    exposure: crate::database::Exposure,
    implemented: &mut BTreeMap<(String, Kind), usize>,
    members: &mut Vec<String>,
) -> Result<(), Error> {
    resolved.validate_scopes()?;
    attributes(member.attributes.as_ref(), &["SecureContext", "SameObject"])?;
    attributes(member.type_.attributes.as_ref(), &[])?;
    if member.modifier.is_some() {
        return Err(Error("JS special attributes are not supported yet".into()));
    }
    let name = member.identifier.0;
    signature(implemented, name, Kind::Get, 0)?;
    let lowered = js_type(database, &member.type_.type_)?;
    let result = output(database, &lowered, "value")?;
    members.push(member_descriptor(
        name,
        "get",
        0,
        "args => []",
        &format!("(value, realm) => {result}"),
        exposure,
    ));
    if member.readonly.is_none() {
        signature(implemented, name, Kind::Set, 1)?;
        let converted = input(&lowered, "args[0]", false)?;
        members.push(member_descriptor(
            name,
            "set",
            1,
            &format!("args => [{converted}]"),
            "(value, realm) => undefined",
            exposure,
        ));
    }
    Ok(())
}

fn lower_operation_member(
    database: &Database<'_>,
    member: &weedle::interface::OperationInterfaceMember<'_>,
    resolved: &crate::database::Member<'_>,
    exposure: crate::database::Exposure,
    implemented: &mut BTreeMap<(String, Kind), usize>,
    members: &mut Vec<String>,
) -> Result<(), Error> {
    resolved.validate_scopes()?;
    attributes(member.attributes.as_ref(), &["NewObject", "SecureContext"])?;
    if member.modifier.is_some() || member.special.is_some() {
        return Err(Error("JS special operations are not supported yet".into()));
    }
    let name = member
        .identifier
        .as_ref()
        .ok_or_else(|| Error("JS operations must be named".into()))?
        .0;
    signature(implemented, name, Kind::Method, member.args.body.list.len())?;
    let (required, conversion) = arguments(database, &member.args.body.list)?;
    let result = match &member.return_type {
        weedle::types::ReturnType::Undefined(_) => "undefined".into(),
        weedle::types::ReturnType::Type(type_) => {
            let lowered = js_type(database, type_)?;
            output(database, &lowered, "value")?
        }
    };
    members.push(member_descriptor(
        name,
        "method",
        required,
        &format!("args => {conversion}"),
        &format!("(value, realm) => {result}"),
        exposure,
    ));
    Ok(())
}

fn member_descriptor(
    name: &str,
    kind: &str,
    count: usize,
    convert: &str,
    result: &str,
    exposure: crate::database::Exposure,
) -> String {
    format!(
        "{{ name: {name:?}, kind: {kind:?}, count: {count}, convert: {convert}, result: {result}, exposure: {{ secureContext: {} }} }}",
        exposure.secure_context
    )
}

enum JsType {
    Native(ReturnType),
    /// `ArrayBufferView`: any typed array or `DataView`
    /// (<https://webidl.spec.whatwg.org/#ArrayBufferView>).
    ArrayBufferView,
}

fn js_type(database: &Database<'_>, type_: &weedle::types::Type<'_>) -> Result<JsType, Error> {
    if matches!(
        type_,
        weedle::types::Type::Single(weedle::types::SingleType::NonAny(
            weedle::types::NonAnyType::ArrayBufferView(item)
        )) if item.q_mark.is_none()
    ) {
        return Ok(JsType::ArrayBufferView);
    }
    native_type(database, type_, &mut BTreeSet::new()).map(JsType::Native)
}

fn signature(
    implemented: &mut BTreeMap<(String, Kind), usize>,
    name: &str,
    kind: Kind,
    count: usize,
) -> Result<(), Error> {
    if implemented.remove(&(name.into(), kind)) == Some(count) {
        Ok(())
    } else {
        Err(Error(format!(
            "{name}: JS algorithm signature does not match IDL {kind:?}"
        )))
    }
}

fn attributes<'a>(
    list: Option<&'a ExtendedAttributeList<'a>>,
    supported: &[&str],
) -> Result<Vec<&'a str>, Error> {
    let mut names = Vec::new();
    if let Some(list) = list {
        for attribute in &list.body.list {
            let name = match attribute {
                ExtendedAttribute::NoArgs(item) => item.0.0,
                ExtendedAttribute::WildCard(item) => item.identifier.0,
                ExtendedAttribute::Ident(item) => item.lhs_identifier.0,
                ExtendedAttribute::IdentList(item) => item.identifier.0,
                _ => {
                    return Err(Error(
                        "JS extended attribute form is not supported yet".into(),
                    ));
                }
            };
            if !supported.contains(&name)
                || (name != "Exposed" && !matches!(attribute, ExtendedAttribute::NoArgs(_)))
            {
                return Err(Error(format!(
                    "JS IDL semantics are not supported yet: {attribute:?}"
                )));
            }
            names.push(name);
        }
    }
    Ok(names)
}

fn arguments(
    database: &Database<'_>,
    arguments: &[weedle::argument::Argument<'_>],
) -> Result<(usize, String), Error> {
    let mut conversions = Vec::new();
    let mut required = 0;
    let mut optional = false;
    for (index, argument) in arguments.iter().enumerate() {
        let weedle::argument::Argument::Single(argument) = argument else {
            return Err(Error("JS variadic arguments are not supported yet".into()));
        };
        attributes(argument.attributes.as_ref(), &["AllowShared"])?;
        attributes(argument.type_.attributes.as_ref(), &["AllowShared"])?;
        let allow_shared = argument.attributes.iter().chain(argument.type_.attributes.iter())
            .any(|attributes| attributes.body.list.iter().any(|attribute| matches!(attribute, ExtendedAttribute::NoArgs(item) if item.0.0 == "AllowShared")));
        // Read an argument only when it is present. `args[index]` past the end
        // would otherwise consult the page-controlled `Array.prototype`.
        let present = format!("args.length > {index}");
        let value = format!("{present} ? args[{index}] : undefined");
        let lowered = js_type(database, &argument.type_.type_)?;
        let converted = input(&lowered, &value, allow_shared)?;
        let converted = match (&argument.optional, &argument.default) {
            (None, None) if !optional => {
                required += 1;
                input(&lowered, &format!("args[{index}]"), allow_shared)?
            }
            (Some(_), Some(default)) => {
                optional = true;
                let weedle::literal::DefaultValue::String(default) = &default.value else {
                    return Err(Error("JS argument defaults are not supported yet".into()));
                };
                match &lowered {
                    JsType::Native(ReturnType::String | ReturnType::UsvString) => {}
                    _ => return Err(Error("JS string defaults require string arguments".into())),
                }
                format!(
                    "{present} && args[{index}] !== undefined ? {converted} : {:?}",
                    default.0
                )
            }
            _ => return Err(Error("JS argument optionality is not supported yet".into())),
        };
        conversions.push(converted);
    }
    Ok((required, format!("[{}]", conversions.join(","))))
}

fn input(type_: &JsType, value: &str, allow_shared: bool) -> Result<String, Error> {
    // https://webidl.spec.whatwg.org/#es-type-mapping
    match type_ {
        JsType::ArrayBufferView => Ok(format!("__tbIDLBufferSource({value}, {allow_shared})")),
        // `AllowShared` is meaningless on a copied string; only the argument
        // conversion differs from the native side.
        JsType::Native(ReturnType::String) if !allow_shared => {
            Ok(format!("__tbIDLString({value})"))
        }
        JsType::Native(ReturnType::UsvString) if !allow_shared => {
            Ok(format!("__tbIDLUSVString({value})"))
        }
        JsType::Native(ReturnType::Uint8Array) => {
            Ok(format!("__tbIDLUint8Array({value}, {allow_shared})"))
        }
        JsType::Native(other) => Err(Error(format!(
            "JS argument type is not supported yet: {other:?}"
        ))),
    }
}

fn output(database: &Database<'_>, type_: &JsType, value: &str) -> Result<String, Error> {
    match type_ {
        // The conversion of an IDL `ArrayBufferView` back to JS is the object
        // the argument conversion already validated
        // (<https://webidl.spec.whatwg.org/#es-buffer-source-types>).
        JsType::ArrayBufferView => Ok(value.to_owned()),
        JsType::Native(ReturnType::String) => Ok(format!("__tbIDLResult({value}, 'string')")),
        JsType::Native(ReturnType::Uint8Array) => {
            Ok(format!("__tbIDLRealmUint8Array({value}, false, realm)"))
        }
        // https://webidl.spec.whatwg.org/#es-to-js-dictionary
        JsType::Native(ReturnType::Dictionary(name)) => {
            let dictionary = database.dictionary(name)?;
            attributes(dictionary.attributes.as_ref(), &[])?;
            if dictionary.inheritance.is_some() {
                return Err(Error(
                    "JS result dictionary inheritance is not supported yet".into(),
                ));
            }
            let mut fields = Vec::new();
            for member in dictionary.members.body {
                attributes(member.attributes.as_ref(), &[])?;
                attributes(member.type_.attributes.as_ref(), &[])?;
                let lowered = js_type(database, &member.type_.type_)?;
                if member.required.is_some()
                    || member.default.is_some()
                    || !matches!(lowered, JsType::Native(ReturnType::UnsignedLongLong))
                {
                    return Err(Error(
                        "JS result dictionary fields are not supported yet".into(),
                    ));
                }
                fields.push(member.identifier.0);
            }
            fields.sort_unstable();
            if fields.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(Error("duplicate JS result dictionary field".into()));
            }
            Ok(format!(
                "__tbIDLResultDictionary({value}, [{}])",
                fields
                    .iter()
                    .map(|field| format!("{field:?}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ))
        }
        JsType::Native(other) => Err(Error(format!(
            "JS result type is not supported yet: {other:?}"
        ))),
    }
}
