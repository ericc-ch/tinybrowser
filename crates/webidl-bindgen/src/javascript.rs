use std::collections::{BTreeMap, BTreeSet};

use oxc_allocator::Allocator;
use oxc_ast::ast::{self, ClassElement, MethodDefinitionKind};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};
use weedle::attribute::{ExtendedAttribute, ExtendedAttributeList};
use weedle::interface::InterfaceMember;
use weedle::types::{NonAnyType, SingleType, Type};

use crate::database::Database;
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
                        .map_or(0, |prefix| prefix.bytes().filter(|byte| *byte == b'\n').count() + 1);
                    self.error = Some(Error(format!("line {line}: {error}")));
                }
            }
        }
        walk::walk_call_expression(self, call);
    }
}

impl Discovery<'_, '_, '_> {
    fn installation(&mut self, call: &ast::CallExpression<'_>) -> Result<String, Error> {
        let [ast::Argument::ClassExpression(class)] = call.arguments.as_slice() else {
            return Err(Error(
                "interface installation requires one named class expression".into(),
            ));
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
        attributes(declaration.attributes.as_ref(), &["Exposed"])?;
        let exposed = declaration.attributes.as_ref().is_some_and(|attributes| {
            attributes
                .body
                .list
                .iter()
                .any(|attribute| match attribute {
                    ExtendedAttribute::WildCard(item) => item.identifier.0 == "Exposed",
                    ExtendedAttribute::Ident(item) => {
                        item.lhs_identifier.0 == "Exposed" && item.rhs.0 == "Window"
                    }
                    ExtendedAttribute::IdentList(item) => {
                        item.identifier.0 == "Exposed"
                            && item.list.body.list.iter().any(|item| item.0 == "Window")
                    }
                    _ => false,
                })
        });
        if !exposed || declaration.parent.is_some() {
            return Err(Error(
                "JS interface exposure or inheritance is not supported yet".into(),
            ));
        }
        let (constructor, members) =
            lower_members(self.database, &declaration, implementation_methods(class)?)?;
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
) -> Result<(Option<String>, Vec<String>), Error> {
    let provided: BTreeSet<_> = implemented.keys().cloned().collect();
    let mut constructor = None;
    let mut members = Vec::new();
    for member in &declaration.members {
        let resolved = member;
        match &member.declaration {
            InterfaceMember::Constructor(member)
                if provided.contains(&("constructor".into(), Kind::Constructor)) =>
            {
                resolved.validate_scopes()?;
                attributes(member.attributes.as_ref(), &[])?;
                if constructor.is_some() {
                    return Err(Error(
                        "JS constructor overloads are not supported yet".into(),
                    ));
                }
                let (required, conversion) = arguments(&member.args.body.list)?;
                signature(
                    &mut implemented,
                    "constructor",
                    Kind::Constructor,
                    member.args.body.list.len(),
                )?;
                constructor = Some(format!("[{required}, args => {conversion}]"));
            }
            InterfaceMember::Attribute(member)
                if provided.contains(&(member.identifier.0.into(), Kind::Get))
                    || provided.contains(&(member.identifier.0.into(), Kind::Set)) =>
            {
                resolved.validate_scopes()?;
                attributes(member.attributes.as_ref(), &[])?;
                attributes(member.type_.attributes.as_ref(), &[])?;
                if member.modifier.is_some() {
                    return Err(Error("JS special attributes are not supported yet".into()));
                }
                let name = member.identifier.0;
                signature(&mut implemented, name, Kind::Get, 0)?;
                let result = output(database, &member.type_.type_, "value")?;
                members.push(format!(
                    "[{name:?}, 'get', 0, args => [], (value, realm) => {result}]"
                ));
                if member.readonly.is_none() {
                    signature(&mut implemented, name, Kind::Set, 1)?;
                    let converted = input(&member.type_.type_, "args[0]", false)?;
                    members.push(format!(
                        "[{name:?}, 'set', 1, args => [{converted}], (value, realm) => undefined]"
                    ));
                }
            }
            InterfaceMember::Operation(member)
                if member
                    .identifier
                    .is_some_and(|name| provided.contains(&(name.0.into(), Kind::Method))) =>
            {
                resolved.validate_scopes()?;
                attributes(member.attributes.as_ref(), &["NewObject"])?;
                if member.modifier.is_some() || member.special.is_some() {
                    return Err(Error("JS special operations are not supported yet".into()));
                }
                let name = member
                    .identifier
                    .as_ref()
                    .ok_or_else(|| Error("JS operations must be named".into()))?
                    .0;
                signature(
                    &mut implemented,
                    name,
                    Kind::Method,
                    member.args.body.list.len(),
                )?;
                let (required, conversion) = arguments(&member.args.body.list)?;
                let result = match &member.return_type {
                    weedle::types::ReturnType::Undefined(_) => "undefined".into(),
                    weedle::types::ReturnType::Type(type_) => output(database, type_, "value")?,
                };
                members.push(format!(
                    "[{name:?}, 'method', {required}, args => {conversion}, (value, realm) => {result}]"
                ));
            }
            _ => {}
        }
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

fn attributes(list: Option<&ExtendedAttributeList<'_>>, supported: &[&str]) -> Result<(), Error> {
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
        }
    }
    Ok(())
}

fn arguments(arguments: &[weedle::argument::Argument<'_>]) -> Result<(usize, String), Error> {
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
        let converted = input(&argument.type_.type_, &value, allow_shared)?;
        let converted = match (&argument.optional, &argument.default) {
            (None, None) if !optional => {
                required += 1;
                input(
                    &argument.type_.type_,
                    &format!("args[{index}]"),
                    allow_shared,
                )?
            }
            (Some(_), Some(default)) => {
                optional = true;
                let weedle::literal::DefaultValue::String(default) = &default.value else {
                    return Err(Error("JS argument defaults are not supported yet".into()));
                };
                match &argument.type_.type_ {
                    Type::Single(SingleType::NonAny(
                        NonAnyType::DOMString(_) | NonAnyType::USVString(_),
                    )) => {}
                    _ => return Err(Error("JS string defaults require string arguments".into())),
                }
                format!("{present} && args[{index}] !== undefined ? {converted} : {:?}", default.0)
            }
            _ => return Err(Error("JS argument optionality is not supported yet".into())),
        };
        conversions.push(converted);
    }
    Ok((required, format!("[{}]", conversions.join(","))))
}

fn input(type_: &Type<'_>, value: &str, allow_shared: bool) -> Result<String, Error> {
    // https://webidl.spec.whatwg.org/#es-type-mapping
    match type_ {
        Type::Single(SingleType::NonAny(NonAnyType::DOMString(item)))
            if item.q_mark.is_none() && !allow_shared =>
        {
            Ok(format!("__tbIDLString({value})"))
        }
        Type::Single(SingleType::NonAny(NonAnyType::USVString(item)))
            if item.q_mark.is_none() && !allow_shared =>
        {
            Ok(format!("__tbIDLUSVString({value})"))
        }
        Type::Single(SingleType::NonAny(NonAnyType::Uint8Array(item))) if item.q_mark.is_none() => {
            Ok(format!("__tbIDLUint8Array({value}, {allow_shared})"))
        }
        _ => Err(Error(format!(
            "JS argument type is not supported yet: {type_:?}"
        ))),
    }
}

fn output(database: &Database<'_>, type_: &Type<'_>, value: &str) -> Result<String, Error> {
    // https://webidl.spec.whatwg.org/#es-to-js-dictionary
    match type_ {
        Type::Single(SingleType::NonAny(NonAnyType::DOMString(item))) if item.q_mark.is_none() => {
            Ok(format!("__tbIDLResult({value}, 'string')"))
        }
        Type::Single(SingleType::NonAny(NonAnyType::Uint8Array(item))) if item.q_mark.is_none() => {
            Ok(format!("__tbIDLRealmUint8Array({value}, false, realm)"))
        }
        Type::Single(SingleType::NonAny(NonAnyType::Identifier(item))) if item.q_mark.is_none() => {
            let dictionary = database.dictionary(item.type_.0)?;
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
                if member.required.is_some()
                    || member.default.is_some()
                    || !matches!(member.type_.type_, Type::Single(SingleType::NonAny(NonAnyType::Integer(integer))) if integer.q_mark.is_none() && matches!(integer.type_, weedle::types::IntegerType::LongLong(integer) if integer.unsigned.is_some()))
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
        _ => Err(Error(format!(
            "JS result type is not supported yet: {type_:?}"
        ))),
    }
}
