use std::collections::{BTreeMap, BTreeSet};

use weedle::interface::{InterfaceMember, StringifierOrInheritOrStatic, StringifierOrStatic};
use weedle::mixin::MixinMember;
use weedle::{Definition, Definitions, Parse};

use crate::{Error, Source};

pub(crate) struct Database<'idl> {
    definitions: BTreeMap<&'idl str, Definition<'idl>>,
    partials: BTreeMap<&'idl str, Vec<Definition<'idl>>>,
    includes: BTreeMap<&'idl str, BTreeSet<&'idl str>>,
}

pub(crate) struct Interface<'idl> {
    pub(crate) name: &'idl str,
    pub(crate) attributes: Option<weedle::attribute::ExtendedAttributeList<'idl>>,
    pub(crate) parent: Option<&'idl str>,
    pub(crate) members: Vec<InterfaceMember<'idl>>,
}

impl<'idl> Database<'idl> {
    pub(crate) fn parse(sources: &[Source<'idl>]) -> Result<Self, Error> {
        let mut result = Self {
            definitions: BTreeMap::new(),
            partials: BTreeMap::new(),
            includes: BTreeMap::new(),
        };
        for source in sources {
            let (remaining, definitions) = Definitions::parse(source.text)
                .map_err(|error| Error(format!("{}: invalid Web IDL: {error}", source.name)))?;
            if !remaining.trim().is_empty() {
                let line = source.text[..source.text.len() - remaining.len()]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1;
                let preview: String = remaining.chars().take(120).collect();
                return Err(Error(format!(
                    "{}:{line}: unsupported IDL syntax near {preview:?}",
                    source.name
                )));
            }
            for definition in definitions {
                match definition {
                    Definition::IncludesStatement(include) => {
                        let interface = include.lhs_identifier.0;
                        let mixin = include.rhs_identifier.0;
                        if !result.includes.entry(interface).or_default().insert(mixin) {
                            return Err(Error(format!(
                                "duplicate includes: {interface} includes {mixin}"
                            )));
                        }
                    }
                    Definition::Implements(_) => {
                        return Err(Error(format!(
                            "{}: obsolete implements statement",
                            source.name
                        )));
                    }
                    Definition::PartialInterface(ref item) => {
                        result
                            .partials
                            .entry(item.identifier.0)
                            .or_default()
                            .push(definition);
                    }
                    Definition::PartialInterfaceMixin(ref item) => {
                        result
                            .partials
                            .entry(item.identifier.0)
                            .or_default()
                            .push(definition);
                    }
                    Definition::PartialDictionary(ref item) => {
                        result
                            .partials
                            .entry(item.identifier.0)
                            .or_default()
                            .push(definition);
                    }
                    Definition::PartialNamespace(ref item) => {
                        result
                            .partials
                            .entry(item.identifier.0)
                            .or_default()
                            .push(definition);
                    }
                    definition => {
                        let name = definition_name(&definition)?;
                        if result.definitions.insert(name, definition).is_some() {
                            return Err(Error(format!(
                                "{}: duplicate definition {name}",
                                source.name
                            )));
                        }
                    }
                }
            }
        }
        result.validate_structure()?;
        Ok(result)
    }

    fn validate_structure(&self) -> Result<(), Error> {
        // https://webidl.spec.whatwg.org/#using-mixins-and-partials
        // https://chromium.googlesource.com/chromium/src/+/main/third_party/blink/renderer/bindings/scripts/web_idl/idl_compiler.py
        for (name, partials) in &self.partials {
            let definition = self
                .definitions
                .get(name)
                .ok_or_else(|| Error(format!("partial {name} has no complete definition")))?;
            for partial in partials {
                if !matches!(
                    (definition, partial),
                    (Definition::Interface(_), Definition::PartialInterface(_))
                        | (
                            Definition::InterfaceMixin(_),
                            Definition::PartialInterfaceMixin(_)
                        )
                        | (Definition::Dictionary(_), Definition::PartialDictionary(_))
                        | (Definition::Namespace(_), Definition::PartialNamespace(_))
                ) {
                    return Err(Error(format!(
                        "partial {name} has a different declaration kind"
                    )));
                }
            }
        }
        for (interface, mixins) in &self.includes {
            if !matches!(
                self.definitions.get(interface),
                Some(Definition::Interface(_))
            ) {
                return Err(Error(format!(
                    "includes target {interface} is not an interface"
                )));
            }
            for mixin in mixins {
                if !matches!(
                    self.definitions.get(mixin),
                    Some(Definition::InterfaceMixin(_))
                ) {
                    return Err(Error(format!("included {mixin} is not an interface mixin")));
                }
            }
        }
        for (name, definition) in &self.definitions {
            if parent_name(definition).is_some() {
                let mut visited = BTreeSet::new();
                let mut current = *name;
                loop {
                    if !visited.insert(current) {
                        return Err(Error(format!("inheritance cycle at {current}")));
                    }
                    let item = self.definitions.get(current).ok_or_else(|| {
                        Error(format!("missing inheritance dependency {current}"))
                    })?;
                    if std::mem::discriminant(item) != std::mem::discriminant(definition) {
                        return Err(Error(format!(
                            "{name} inherits a different declaration kind: {current}"
                        )));
                    }
                    let Some(parent) = parent_name(item) else {
                        break;
                    };
                    current = parent;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn interface(&self, name: &str) -> Result<Interface<'idl>, Error> {
        let Some(Definition::Interface(definition)) = self.definitions.get(name) else {
            return Err(Error(format!("no interface declaration for {name}")));
        };
        let mut members = definition.members.body.clone();
        if let Some(partials) = self.partials.get(name) {
            for partial in partials {
                let Definition::PartialInterface(partial) = partial else {
                    return Err(Error(format!("invalid partial interface {name}")));
                };
                members.extend(partial.members.body.iter().cloned());
            }
        }
        if let Some(mixins) = self.includes.get(name) {
            for mixin in mixins {
                let Some(Definition::InterfaceMixin(definition)) = self.definitions.get(mixin)
                else {
                    return Err(Error(format!("invalid mixin {mixin}")));
                };
                members.extend(definition.members.body.iter().map(mixin_member));
                if let Some(partials) = self.partials.get(mixin) {
                    for partial in partials {
                        let Definition::PartialInterfaceMixin(partial) = partial else {
                            return Err(Error(format!("invalid partial mixin {mixin}")));
                        };
                        members.extend(partial.members.body.iter().map(mixin_member));
                    }
                }
            }
        }
        Ok(Interface {
            name: definition.identifier.0,
            attributes: definition.attributes.clone(),
            parent: definition
                .inheritance
                .as_ref()
                .map(|item| item.identifier.0),
            members,
        })
    }

    pub(crate) fn definition(&self, name: &str) -> Option<&Definition<'idl>> {
        self.definitions.get(name)
    }
}

fn definition_name<'idl>(definition: &Definition<'idl>) -> Result<&'idl str, Error> {
    match definition {
        Definition::Callback(item) => Ok(item.identifier.0),
        Definition::CallbackInterface(item) => Ok(item.identifier.0),
        Definition::Interface(item) => Ok(item.identifier.0),
        Definition::InterfaceMixin(item) => Ok(item.identifier.0),
        Definition::Namespace(item) => Ok(item.identifier.0),
        Definition::Dictionary(item) => Ok(item.identifier.0),
        Definition::Enum(item) => Ok(item.identifier.0),
        Definition::Typedef(item) => Ok(item.identifier.0),
        Definition::PartialInterface(_)
        | Definition::PartialInterfaceMixin(_)
        | Definition::PartialDictionary(_)
        | Definition::PartialNamespace(_)
        | Definition::IncludesStatement(_)
        | Definition::Implements(_) => Err(Error("expected a complete named definition".into())),
    }
}

fn parent_name<'idl>(definition: &Definition<'idl>) -> Option<&'idl str> {
    match definition {
        Definition::Interface(item) => item.inheritance.as_ref(),
        Definition::CallbackInterface(item) => item.inheritance.as_ref(),
        Definition::Dictionary(item) => item.inheritance.as_ref(),
        _ => None,
    }
    .map(|item| item.identifier.0)
}

fn mixin_member<'idl>(member: &MixinMember<'idl>) -> InterfaceMember<'idl> {
    match member {
        MixinMember::Const(item) => InterfaceMember::Const(item.clone()),
        MixinMember::Stringifier(item) => InterfaceMember::Stringifier(item.clone()),
        MixinMember::Attribute(item) => {
            InterfaceMember::Attribute(weedle::interface::AttributeInterfaceMember {
                attributes: item.attributes.clone(),
                modifier: item
                    .stringifier
                    .map(StringifierOrInheritOrStatic::Stringifier),
                readonly: item.readonly,
                attribute: item.attribute,
                type_: item.type_.clone(),
                identifier: item.identifier,
                semi_colon: item.semi_colon,
            })
        }
        MixinMember::Operation(item) => {
            InterfaceMember::Operation(weedle::interface::OperationInterfaceMember {
                attributes: item.attributes.clone(),
                modifier: item.stringifier.map(StringifierOrStatic::Stringifier),
                special: None,
                return_type: item.return_type.clone(),
                identifier: item.identifier,
                args: item.args.clone(),
                semi_colon: item.semi_colon,
            })
        }
    }
}
