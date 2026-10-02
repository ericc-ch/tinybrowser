//! Emit tables and a single dispatch function per interface, not per member.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::model::{
    ArgumentArity, Attribute, ConstructorArgumentKind, Dictionary, DictionaryFieldType,
    Enumeration, GetterMapping, Interface, InterfaceKind, Operation, OperationArgument,
    OperationResult, PropertyGetter, PropertyHooks, PrototypeParent, ReturnType, Setter, Union,
    UnionMemberType,
};

pub(crate) fn interface(interface: &Interface) -> TokenStream {
    let contract = &interface.contract;
    let rust = &interface.rust;
    let module = module_name(&interface.name);
    let payload = if interface.has_lifetime {
        quote! { #rust<'js> }
    } else {
        quote! { #rust }
    };
    let (required, constructor_body) = constructor(interface);
    let dictionaries = interface.dictionaries.iter().map(dictionary);
    let enumerations = interface.enumerations.iter().map(enumeration);
    let unions = interface.unions.iter().map(union);
    let tables = member_tables(interface);
    let unscopables = unscopables(interface);
    let (routes, groups) = dispatch_groups(interface, &payload, "dispatch");
    let (alternate_dispatch, alternate_groups) = alternate_dispatch(interface);
    let legacy_code = legacy_codes(interface);
    let definition = definition(interface, &payload, required);
    let conversion_import = if interface.attributes.iter().any(|attribute| {
        matches!(
            attribute.mapping,
            GetterMapping::Field | GetterMapping::Reflect { .. }
        ) || matches!(
            attribute.return_type,
            ReturnType::String
                | ReturnType::UsvString
                | ReturnType::NullableString
                | ReturnType::Boolean
                | ReturnType::UnsignedShort
                | ReturnType::UnsignedLong
                | ReturnType::Long
                | ReturnType::Double
                | ReturnType::Enumeration(_)
        )
    }) {
        quote! { use rquickjs::IntoJs; }
    } else {
        quote! {}
    };
    let constructor_dispatch = match interface.kind {
        InterfaceKind::Complete => quote! {
            if operation.index() == 0 {
                #constructor_body
            }
        },
        InterfaceKind::Partial => quote! {},
    };
    let receiver_check =
        if matches!(interface.kind, InterfaceKind::Partial) && interface.contract.is_some() {
            let name = &interface.name;
            quote! { host::SharedClass::require_interface(&*receiver, &ctx, #name)?; }
        } else if interface.rust == "JsNode" && interface.name != "Node" {
            let name = &interface.name;
            quote! { host::require_node_interface(&ctx, receiver.node_id(), #name)?; }
        } else {
            quote! {}
        };
    quote! {
        pub(super) mod #module {
            use super::#rust;
            use crate::js::bindings::host;
            use rquickjs::{Ctx, Object, Result, Value};
            use rquickjs::function::Params;
            #conversion_import

            #tables
            #contract
            #definition
            #(#dictionaries)*
            #(#enumerations)*
            #(#unions)*
            #unscopables

            // https://webidl.spec.whatwg.org/#es-interface-call
            fn dispatch<'js>(operation: host::Operation, params: &Params<'_, 'js>) -> Result<Value<'js>> {
                let ctx = params.ctx().clone();
                #constructor_dispatch
                #alternate_dispatch
                // https://webidl.spec.whatwg.org/#es-attributes
                // https://webidl.spec.whatwg.org/#es-operations
                let receiver = host::receiver::<#payload>(params)?;
                let receiver = receiver.borrow();
                #receiver_check
                match operation.index() {
                    #(#routes,)*
                    _ => Err(rquickjs::Exception::throw_internal(&ctx, "unknown native operation")),
                }
            }

            #(#groups)*
            #(#alternate_groups)*

            #legacy_code
        }
    }
}

fn member_tables(interface: &Interface) -> TokenStream {
    let attributes_members = interface
        .attributes
        .iter()
        .enumerate()
        .map(|(index, attribute)| {
            let id = index * 2 + 1;
            let name = &attribute.name;
            let setter = if attribute.setter.is_some() {
                let id = id + 1;
                quote! { Some(host::Operation::new(#id)) }
            } else {
                quote! { None }
            };
            quote! { host::Member { name: #name, kind: host::MemberKind::Attribute {
                getter: host::Operation::new(#id), setter: #setter,
            } } }
        });
    let operation_members = interface.operations.iter().enumerate().map(|(index, operation)| {
        let id = interface.attributes.len() * 2 + index + 1;
        let name = &operation.name;
        let length = operation
        .arguments
        .iter()
        .take_while(|argument| argument.arity == ArgumentArity::Required)
        .count();
        quote! { host::Member { name: #name, kind: host::MemberKind::Method { operation: host::Operation::new(#id), length: #length } } }
    });
    let stringifier = interface.stringifier.map(|_| {
        let id = interface.attributes.len() * 2 + interface.operations.len() + 1;
        quote! { host::Member { name: "toString", kind: host::MemberKind::Method { operation: host::Operation::new(#id), length: 0 } } }
    });
    let members = attributes_members
        .chain(operation_members)
        .chain(stringifier);
    let constants = interface.constants.iter().map(|constant| {
        let name = &constant.name;
        let value = constant.value;
        quote! { host::Constant { name: #name, value: #value } }
    });
    quote! {
        const MEMBERS: &[host::Member] = &[#(#members),*];
        const CONSTANTS: &[host::Constant] = &[#(#constants),*];
    }
}

fn alternate_dispatch(interface: &Interface) -> (TokenStream, Vec<TokenStream>) {
    let Some(alternate) = &interface.alternate else {
        return (quote! {}, Vec::new());
    };
    let payload = if interface.alternate_has_lifetime {
        quote! { crate::js::bindings::#alternate<'js> }
    } else {
        quote! { crate::js::bindings::#alternate }
    };
    let (routes, groups) = dispatch_groups(interface, &payload, "alternate");
    (
        quote! {
            if let Ok(receiver) = rquickjs::Class::<#payload>::from_value(&params.this()) {
                let receiver = receiver.borrow();
                return match operation.index() {
                    #(#routes,)*
                    _ => Err(rquickjs::Exception::throw_internal(&ctx, "unknown native operation")),
                };
            }
        },
        groups,
    )
}

fn dispatch_groups(
    interface: &Interface,
    payload: &TokenStream,
    prefix: &str,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut arms = Vec::new();
    for (index, attribute) in interface.attributes.iter().enumerate() {
        arms.push((
            index * 2 + 1,
            false,
            getter_dispatch(index * 2 + 1, attribute, interface),
        ));
        if let Some(setter) = setter_dispatch((index, attribute), interface) {
            arms.push((index * 2 + 2, true, setter));
        }
    }
    for (index, operation) in interface.operations.iter().enumerate() {
        let id = interface.attributes.len() * 2 + index + 1;
        arms.push((id, true, operation_dispatch(id, operation, interface)));
    }
    if let Some(index) = interface.stringifier {
        let id = interface.attributes.len() * 2 + interface.operations.len() + 1;
        arms.push((
            id,
            false,
            getter_dispatch(id, &interface.attributes[index], interface),
        ));
    }
    let mut routes = Vec::new();
    let mut groups = Vec::new();
    // Bound each generated switch while retaining one native entry point and
    // one payload borrow. These are plain Rust functions, not JS callbacks.
    for (index, chunk) in arms.chunks(4).enumerate() {
        let name = format_ident!("{prefix}_group_{index}");
        let first = chunk[0].0;
        let last = chunk[chunk.len() - 1].0;
        let parameters = if chunk.iter().any(|(_, uses_params, _)| *uses_params) {
            format_ident!("params")
        } else {
            format_ident!("_params")
        };
        let bodies = chunk.iter().map(|(_, _, body)| body);
        routes.push(quote! {
            #first..=#last => #name(&ctx, &receiver, operation, params)
        });
        groups.push(quote! {
            fn #name<'js>(
                ctx: &Ctx<'js>,
                receiver: &#payload,
                operation: host::Operation,
                #parameters: &Params<'_, 'js>,
            ) -> Result<Value<'js>> {
                let ctx = ctx.clone();
                match operation.index() {
                    #(#bodies,)*
                    _ => Err(rquickjs::Exception::throw_internal(&ctx, "unknown native operation")),
                }
            }
        });
    }
    (routes, groups)
}

fn definition(interface: &Interface, payload: &TokenStream, required: usize) -> TokenStream {
    let name = &interface.name;
    let property_hooks = indexed_property_hooks(interface);
    let parent = match &interface.parent {
        Some(PrototypeParent::Intrinsic(parent) | PrototypeParent::Interface(parent)) => {
            quote! { Some(#parent) }
        }
        None => quote! { None },
    };
    let constructor_parent = match &interface.parent {
        Some(PrototypeParent::Interface(parent)) => quote! {
            let parent: Object = ctx.globals().get(#parent)?;
            constructor.set_prototype(Some(&parent))?;
        },
        Some(PrototypeParent::Intrinsic(_)) | None => quote! {},
    };
    match interface.kind {
        InterfaceKind::Complete => quote! {
            use rquickjs::class::{JsClass, Readable};
            use rquickjs::function::Constructor;
            impl<'js> JsClass<'js> for #payload {
                const NAME: &'static str = #name;
                type Mutable = Readable;
                #property_hooks

                fn prototype(ctx: &Ctx<'js>) -> Result<Option<Object<'js>>> {
                    let prototype = host::prototype(ctx, #name, #parent, MEMBERS, CONSTANTS, dispatch)?;
                    host::install_unscopables(&prototype, UNSCOPABLES)?;
                    Ok(Some(prototype))
                }

                fn constructor(ctx: &Ctx<'js>) -> Result<Option<Constructor<'js>>> {
                    let prototype = Self::prototype(ctx)?.ok_or_else(|| {
                        rquickjs::Exception::throw_internal(ctx, "native interface has no prototype")
                    })?;
                    let constructor = host::constructor(ctx, #name, #required, &prototype, CONSTANTS,
                        host::HostCall::new(dispatch, host::Operation::new(0)))?;
                    #constructor_parent
                    Ok(Some(constructor))
                }
            }
        },
        InterfaceKind::Partial => {
            let targets: Vec<&str> = if interface.install_targets.is_empty() {
                vec![name.as_str()]
            } else {
                interface
                    .install_targets
                    .iter()
                    .map(String::as_str)
                    .collect()
            };
            quote! {
                pub(crate) fn install(ctx: &Ctx<'_>) -> Result<()> {
                    #(
                        let constructor: Object = ctx.globals().get(#targets)?;
                        let prototype: Object = constructor.get("prototype")?;
                        host::install_members(&prototype, MEMBERS, CONSTANTS, dispatch)?;
                        host::install_unscopables(&prototype, UNSCOPABLES)?;
                    )*
                    Ok(())
                }
            }
        }
    }
}

struct NamedHooks {
    descriptor: TokenStream,
    names: TokenStream,
    define: TokenStream,
    set: TokenStream,
    delete: TokenStream,
}

impl NamedHooks {
    fn none() -> Self {
        Self {
            descriptor: quote! { return Ok(None); },
            names: quote! {},
            define: quote! {},
            set: quote! {},
            delete: quote! { return Ok(true); },
        }
    }

    fn parse(interface: &Interface) -> Self {
        let PropertyHooks::IndexedNamed {
            names,
            unenumerable,
            override_builtins,
        } = &interface.properties
        else {
            return Self::none();
        };
        let named_item = &interface
            .operations
            .iter()
            .find(|operation| operation.getter == Some(PropertyGetter::Named))
            .expect("validated named item")
            .rust;
        let enumerable = !unenumerable;
        // `[LegacyOverrideBuiltIns]`: named properties override built-ins, so
        // no prototype-visibility check
        // (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection>).
        // Plain `HTMLCollection` keeps `[LegacyUnenumerableNamedProperties]` hiding.
        if *override_builtins {
            return Self {
                descriptor: quote! {
                    if receiver.#names(ctx)?.contains(&name) {
                        let value = receiver.#named_item(ctx.clone(), rquickjs::String::from_str(ctx.clone(), &name)?)?;
                        return Ok(Some(rquickjs::class::PropertyDescriptor::new_value(value, true, #enumerable, false)));
                    }
                    return Ok(None);
                },
                names: quote! {
                    for name in this.borrow().#names(ctx)? {
                        if crate::js::bindings::array_index(&name).is_none() {
                            names.push(rquickjs::class::PropertyName {
                                atom: rquickjs::Atom::from_str(ctx.clone(), &name)?,
                                is_enumerable: #enumerable,
                            });
                        }
                    }
                },
                define: quote! {
                    if this.borrow().#names(ctx)?.contains(&name) {
                        return Ok(rquickjs::class::ExoticDefineResult::Handled(false));
                    }
                },
                set: quote! {
                    if object != receiver && crate::js::bindings::array_index(&name).is_none() {
                        return Ok(rquickjs::class::ExoticSetResult::FallthroughSkippingOwnProperty);
                    }
                },
                delete: quote! {
                    return Ok(!this.borrow().#names(ctx)?.contains(&name));
                },
            };
        }
        Self {
            descriptor: quote! {
                if receiver.#names(ctx)?.contains(&name)
                    && crate::js::bindings::named_key_visible(&object, &name)? {
                    let value = receiver.#named_item(ctx.clone(), rquickjs::String::from_str(ctx.clone(), &name)?)?;
                    return Ok(Some(rquickjs::class::PropertyDescriptor::new_value(value, true, #enumerable, false)));
                }
                return Ok(None);
            },
            names: quote! {
                for name in this.borrow().#names(ctx)? {
                    if crate::js::bindings::array_index(&name).is_none()
                        && crate::js::bindings::named_key_visible(&object, &name)? {
                        names.push(rquickjs::class::PropertyName {
                            atom: rquickjs::Atom::from_str(ctx.clone(), &name)?,
                            is_enumerable: #enumerable,
                        });
                    }
                }
            },
            define: quote! {
                if this.borrow().#names(ctx)?.contains(&name) {
                    return Ok(rquickjs::class::ExoticDefineResult::Handled(false));
                }
            },
            set: quote! {
                if object != receiver && crate::js::bindings::array_index(&name).is_none() {
                    return Ok(rquickjs::class::ExoticSetResult::FallthroughSkippingOwnProperty);
                }
            },
            delete: quote! {
                return Ok(!this.borrow().#names(ctx)?.contains(&name)
                    || !crate::js::bindings::named_key_visible(&object, &name)?);
            },
        }
    }
}

fn indexed_property_hooks(interface: &Interface) -> TokenStream {
    if !matches!(
        interface.properties,
        PropertyHooks::Indexed | PropertyHooks::IndexedNamed { .. }
    ) {
        return quote! {};
    }
    let length = &interface
        .attributes
        .iter()
        .find(|attribute| attribute.name == "length")
        .expect("validated indexed length")
        .rust;
    let item = interface
        .operations
        .iter()
        .find(|operation| operation.getter == Some(PropertyGetter::Indexed))
        .expect("validated indexed item");
    let named = NamedHooks::parse(interface);
    let object = match &interface.properties {
        PropertyHooks::IndexedNamed {
            override_builtins: true,
            ..
        } => format_ident!("_object"),
        PropertyHooks::IndexedNamed { .. } => format_ident!("object"),
        _ => format_ident!("_object"),
    };
    let define_receiver = if matches!(interface.properties, PropertyHooks::IndexedNamed { .. })
        || interface.indexed_setter.is_some()
    {
        format_ident!("this")
    } else {
        format_ident!("_this")
    };
    let mutable_names = if matches!(interface.properties, PropertyHooks::IndexedNamed { .. }) {
        quote! { mut }
    } else {
        quote! {}
    };
    indexed_hooks_body(
        length,
        item,
        interface.indexed_setter.as_ref(),
        &named,
        &object,
        &define_receiver,
        &mutable_names,
    )
}

struct SetterTokens {
    define_value: proc_macro2::Ident,
    define_is_data: proc_macro2::Ident,
    indexed_define: TokenStream,
    set_receiver: proc_macro2::Ident,
    set_value: proc_macro2::Ident,
    indexed_set: TokenStream,
    writable: bool,
}

impl SetterTokens {
    fn parse(setter: Option<&crate::model::IndexedSetter>) -> Self {
        let Some(setter) = setter else {
            return Self {
                define_value: format_ident!("_value"),
                define_is_data: format_ident!("_is_data"),
                indexed_define: quote! {
                    if crate::js::bindings::array_index(&name).is_some() {
                        return Ok(rquickjs::class::ExoticDefineResult::Handled(false));
                    }
                },
                set_receiver: format_ident!("_this"),
                set_value: format_ident!("_value"),
                indexed_set: quote! {},
                writable: false,
            };
        };
        let method = &setter.rust;
        let call = quote! {
            let receiver = this.borrow();
            receiver.#method(ctx.clone(), index, value)?;
        };
        let call = if setter.reactions {
            quote! {
                crate::js::reactions::with_reactions(ctx, || {
                    #call
                    Ok(Value::new_undefined(ctx.clone()))
                })?;
            }
        } else {
            quote! { #call }
        };
        Self {
            define_value: format_ident!("value"),
            define_is_data: format_ident!("is_data"),
            indexed_define: quote! {
                if let Some(index) = crate::js::bindings::array_index(&name) {
                    if !is_data {
                        return Ok(rquickjs::class::ExoticDefineResult::Handled(false));
                    }
                    #call
                    return Ok(rquickjs::class::ExoticDefineResult::Handled(true));
                }
            },
            set_receiver: format_ident!("this"),
            set_value: format_ident!("value"),
            indexed_set: quote! {
                if let Some(index) = crate::js::bindings::array_index(&name) {
                    if object != receiver {
                        return Ok(rquickjs::class::ExoticSetResult::Fallthrough);
                    }
                    #call
                    return Ok(rquickjs::class::ExoticSetResult::Handled(true));
                }
            },
            writable: true,
        }
    }
}

fn indexed_hooks_body(
    length: &proc_macro2::Ident,
    item: &Operation,
    setter: Option<&crate::model::IndexedSetter>,
    hooks: &NamedHooks,
    object: &proc_macro2::Ident,
    define_receiver: &proc_macro2::Ident,
    mutable_names: &TokenStream,
) -> TokenStream {
    let tokens = SetterTokens::parse(setter);
    indexed_hooks_with_tokens(
        length,
        item,
        &tokens,
        hooks,
        object,
        define_receiver,
        mutable_names,
    )
}

fn indexed_hooks_with_tokens(
    length: &proc_macro2::Ident,
    item: &Operation,
    tokens: &SetterTokens,
    hooks: &NamedHooks,
    object: &proc_macro2::Ident,
    define_receiver: &proc_macro2::Ident,
    mutable_names: &TokenStream,
) -> TokenStream {
    let descriptor = &hooks.descriptor;
    let supported = &hooks.names;
    let define = &hooks.define;
    let set = &hooks.set;
    let delete = &hooks.delete;
    // Indexed properties are writable only when an indexed setter exists
    // (<https://webidl.spec.whatwg.org/#legacy-platform-object-getownproperty>).
    let writable = tokens.writable;
    let define_value = &tokens.define_value;
    let define_is_data = &tokens.define_is_data;
    let indexed_define = &tokens.indexed_define;
    let set_receiver = &tokens.set_receiver;
    let set_value = &tokens.set_value;
    let indexed_set = &tokens.indexed_set;
    let method = &item.rust;
    let value = if matches!(item.result, OperationResult::NullableString) {
        quote! {
            let value = receiver.#method(ctx.clone(), index)?
                .map_or_else(|| Value::new_null(ctx.clone()), rquickjs::String::into_value);
        }
    } else {
        quote! { let value = rquickjs::IntoJs::into_js(receiver.#method(ctx.clone(), index)?, ctx)?; }
    };
    quote! {
        const KIND: rquickjs::class::ClassKind = rquickjs::class::ClassKind::Exotic;
        const EXOTIC_HOOKS: rquickjs::class::ExoticHooks = rquickjs::class::ExoticHooks {
            get: false, has: false, set: true, delete: true, define_own_property: true,
            get_own_property: true, get_own_property_names: true,
        };

        // https://webidl.spec.whatwg.org/#legacy-platform-object-getownproperty
        fn exotic_get_own_property(
            this: &rquickjs::class::JsCell<'js, Self>, ctx: &Ctx<'js>,
            atom: rquickjs::Atom<'js>, #object: Value<'js>,
        ) -> Result<Option<rquickjs::class::PropertyDescriptor<'js>>> {
            let Some(name) = crate::js::bindings::atom_name(ctx, &atom) else { return Ok(None); };
            let receiver = this.borrow();
            let Some(index) = crate::js::bindings::array_index(&name) else { #descriptor };
            if index as usize >= receiver.#length(ctx)? { return Ok(None); }
            #value
            Ok(Some(rquickjs::class::PropertyDescriptor::new_value(value, true, true, #writable)))
        }

        // https://webidl.spec.whatwg.org/#legacy-platform-object-ownpropertykeys
        fn exotic_get_own_property_names(
            this: &rquickjs::class::JsCell<'js, Self>, ctx: &Ctx<'js>, #object: Value<'js>,
        ) -> Result<Vec<rquickjs::class::PropertyName<'js>>> {
            let #mutable_names names = (0..this.borrow().#length(ctx)?)
                .map(|index| Ok(rquickjs::class::PropertyName {
                    atom: rquickjs::Atom::from_u32(ctx.clone(), u32::try_from(index)
                        .map_err(|_| rquickjs::Exception::throw_range(ctx, "collection index too large"))?)?,
                    is_enumerable: true,
                }))
                .collect::<Result<Vec<_>>>()?;
            #supported
            Ok(names)
        }

        // https://webidl.spec.whatwg.org/#legacy-platform-object-defineownproperty
        fn exotic_define_own_property(
            #define_receiver: &rquickjs::class::JsCell<'js, Self>, ctx: &Ctx<'js>,
            atom: rquickjs::Atom<'js>, #define_value: Value<'js>, #define_is_data: bool,
        ) -> Result<rquickjs::class::ExoticDefineResult> {
            let Some(name) = crate::js::bindings::atom_name(ctx, &atom) else {
                return Ok(rquickjs::class::ExoticDefineResult::Fallthrough);
            };
            #indexed_define
            #define
            Ok(rquickjs::class::ExoticDefineResult::Fallthrough)
        }

        // https://webidl.spec.whatwg.org/#legacy-platform-object-set
        fn exotic_set_property(
            #set_receiver: &rquickjs::class::JsCell<'js, Self>, ctx: &Ctx<'js>, atom: rquickjs::Atom<'js>,
            object: Value<'js>, receiver: Value<'js>, #set_value: Value<'js>,
        ) -> Result<rquickjs::class::ExoticSetResult> {
            let Some(name) = crate::js::bindings::atom_name(ctx, &atom) else {
                return Ok(rquickjs::class::ExoticSetResult::Fallthrough);
            };
            #indexed_set
            #set
            Ok(crate::js::bindings::reject_indexed_write(&name, &object, &receiver))
        }

        // https://webidl.spec.whatwg.org/#legacy-platform-object-delete
        fn exotic_delete_property(
            this: &rquickjs::class::JsCell<'js, Self>, ctx: &Ctx<'js>,
            atom: rquickjs::Atom<'js>, #object: Value<'js>,
        ) -> Result<bool> {
            let Some(name) = crate::js::bindings::atom_name(ctx, &atom) else { return Ok(true); };
            let Some(index) = crate::js::bindings::array_index(&name) else { #delete };
            Ok(index as usize >= this.borrow().#length(ctx)?)
        }
    }
}

fn method_call(interface: &Interface, method: &syn::Ident, arguments: &TokenStream) -> TokenStream {
    if interface.contract.is_some() {
        let contract = format_ident!("{}", interface.name);
        quote! { #contract::#method(receiver, #arguments) }
    } else {
        quote! { receiver.#method(#arguments) }
    }
}

fn getter_dispatch(id: usize, getter: &Attribute, interface: &Interface) -> TokenStream {
    let method = &getter.rust;
    // Operation dispatch always hands over an owned `Ctx`; attributes borrow it
    // unless the interface opts into owned contexts for hand-written getters.
    let ctx_arg = if interface.ctx_mode.is_owned() {
        quote! { ctx.clone() }
    } else {
        quote! { &ctx }
    };
    let call = method_call(interface, method, &ctx_arg);
    let body = match &getter.mapping {
        GetterMapping::Field => quote! { receiver.#method.clone().into_js(&ctx) },
        // `[Reflect]`: generated content-attribute access needs no trait
        // method. The receiver check already ran, so `node_id` targets the
        // branded element.
        GetterMapping::Reflect { content } => match getter.return_type {
            ReturnType::String => quote! {
                host::reflect_string(&ctx, receiver.node_id(), #content)?.into_js(&ctx)
            },
            ReturnType::Boolean => quote! {
                host::reflect_bool(&ctx, receiver.node_id(), #content)?.into_js(&ctx)
            },
            _ => unreachable!("validated reflect mapping"),
        },
        GetterMapping::Method => match getter.return_type {
            // `[RustValue]`: the method returns the platform value directly.
            ReturnType::Value => quote! { #call },
            ReturnType::String => {
                quote! { let result = #call?; result.into_js(&ctx) }
            }
            ReturnType::UsvString => quote! {
                let result = #call?;
                let result = result.to_string_lossy();
                result.into_js(&ctx)
            },
            ReturnType::Boolean => quote! {
                let result: bool = #call?;
                result.into_js(&ctx)
            },
            ReturnType::UnsignedShort => {
                quote! { let result: u16 = #call?; result.into_js(&ctx) }
            }
            ReturnType::Long => {
                quote! { let result: i32 = #call?; result.into_js(&ctx) }
            }
            ReturnType::UnsignedLong => quote! {
                let result = #call?;
                result.into_js(&ctx)
            },
            ReturnType::NullableString => quote! {
                let result: Option<rquickjs::String> = #call?;
                match result {
                    Some(result) => result.into_js(&ctx),
                    None => Ok(Value::new_null(ctx)),
                }
            },
            ReturnType::Double => quote! {
                // https://webidl.spec.whatwg.org/#idl-DOMHighResTimeStamp
                let result: f64 = #call?;
                result.into_js(&ctx)
            },
            ReturnType::NullableNode | ReturnType::NodeList | ReturnType::PlatformObject => {
                quote! { #call }
            }
            ReturnType::Enumeration(_) => quote! {
                let result = #call?;
                rquickjs::String::from_str(ctx.clone(), result.as_str()).map(rquickjs::IntoJs::into_js)
            },
            ReturnType::Node
            | ReturnType::Callback
            | ReturnType::Dictionary(_)
            | ReturnType::Union(..)
            | ReturnType::InterfaceSequence
            | ReturnType::StringSequence
            | ReturnType::NullableDocumentType => {
                unreachable!("validated field mapping")
            }
        },
    };
    quote! { #id => { #body } }
}

fn setter_dispatch(
    (index, attribute): (usize, &Attribute),
    interface: &Interface,
) -> Option<TokenStream> {
    let setter = attribute.setter.as_ref()?;
    let id = index * 2 + 2;
    let ctx_arg = if interface.ctx_mode.is_owned() {
        quote! { ctx.clone() }
    } else {
        quote! { &ctx }
    };
    let (method, from_js) = match setter {
        Setter::Method { rust, from_js } => (rust, from_js),
        Setter::Reflect { content } => {
            return Some(reflect_setter(index, attribute, content));
        }
        Setter::PutForwards { target, nullable } => {
            let name = &attribute.name;
            return Some(quote! {
                #id => {
                    // https://webidl.spec.whatwg.org/#es-attributes
                    host::put_forwards(params, #name, #target, #nullable)
                }
            });
        }
    };
    let convert = if let Some(path) = from_js {
        // https://webidl.spec.whatwg.org/#es-type-mapping
        quote! {
            <super::#path as rquickjs::FromJs>::from_js(
                &ctx,
                params.arg(0).unwrap_or_else(|| Value::new_undefined(ctx.clone())),
            )?
        }
    } else {
        match &attribute.return_type {
            ReturnType::String if attribute.legacy_null_to_empty => quote! {
                host::legacy_null_string_argument(params, 0)?
            },
            ReturnType::String => quote! { host::string_argument(params, 0, None)? },
            ReturnType::NullableString => quote! { host::nullable_string_argument(params, 0)? },
            // https://webidl.spec.whatwg.org/#es-boolean
            ReturnType::Boolean => quote! { host::boolean_argument(params, 0)? },
            // https://webidl.spec.whatwg.org/#es-unsigned-long
            ReturnType::UnsignedLong => quote! {
                {
                    let value = params.arg(0).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                    let converted: rquickjs::Coerced<i32> = rquickjs::FromJs::from_js(&ctx, value)?;
                    converted.0.cast_unsigned()
                }
            },
            // https://webidl.spec.whatwg.org/#es-long
            ReturnType::Long => quote! {
                {
                    let value = params.arg(0).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                    let converted: rquickjs::Coerced<i32> = rquickjs::FromJs::from_js(&ctx, value)?;
                    converted.0
                }
            },
            // https://webidl.spec.whatwg.org/#es-double
            ReturnType::Double => quote! {
                {
                    let value = params.arg(0).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                    let converted: rquickjs::Coerced<f64> = rquickjs::FromJs::from_js(&ctx, value)?;
                    converted.0
                }
            },
            // `[RustValue]`: the platform setter converts the raw argument.
            ReturnType::Value => quote! {
                params.arg(0).unwrap_or_else(|| Value::new_undefined(ctx.clone()))
            },
            ReturnType::Enumeration(name) => {
                let name = format_ident!("{name}");
                quote! {
                    {
                        let value = params.arg(0).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                        #name::from_value(&ctx, value)?
                    }
                }
            }
            _ => unreachable!("validated string, boolean, integer, double, or value setter"),
        }
    };
    let call = method_call(interface, method, &quote! { #ctx_arg, value });
    let body = quote! {
        #call?;
        Ok(Value::new_undefined(ctx.clone()))
    };
    let body = if attribute.reactions {
        quote! { crate::js::reactions::with_reactions(&ctx, || { #body }) }
    } else {
        body
    };
    Some(quote! {
        #id => {
            // https://webidl.spec.whatwg.org/#es-attributes
            // Chromium and Firefox reject an omitted setter argument;
            // WebIDL instead converts undefined, so follow that algorithm.
            let value = #convert;
            #body
        }
    })
}

/// A `[Reflect]` setter: convert the value, then write the content
/// attribute through the shared helper. Reactions wrap the write, like any
/// setter.
fn reflect_setter(index: usize, attribute: &Attribute, content: &str) -> TokenStream {
    let id = index * 2 + 2;
    let convert = match attribute.return_type {
        ReturnType::String | ReturnType::UsvString => {
            quote! { host::string_argument(params, 0, None)? }
        }
        // https://webidl.spec.whatwg.org/#es-boolean
        ReturnType::Boolean => quote! { host::boolean_argument(params, 0)? },
        _ => unreachable!("validated reflect mapping"),
    };
    let write = match attribute.return_type {
        ReturnType::String | ReturnType::UsvString => quote! {
            host::reflect_set_string(&ctx, receiver.node_id(), #content, &value)?;
        },
        ReturnType::Boolean => quote! {
            host::reflect_set_bool(&ctx, receiver.node_id(), #content, value)?;
        },
        _ => unreachable!("validated reflect mapping"),
    };
    let body = quote! {
        #write
        Ok(Value::new_undefined(ctx.clone()))
    };
    let body = if attribute.reactions {
        quote! { crate::js::reactions::with_reactions(&ctx, || { #body }) }
    } else {
        body
    };
    quote! {
        #id => {
            // https://webidl.spec.whatwg.org/#es-attributes
            let value = #convert;
            #body
        }
    }
}

fn operation_dispatch(id: usize, operation: &Operation, interface: &Interface) -> TokenStream {
    let method = &operation.rust;
    let required = operation
        .arguments
        .iter()
        .take_while(|argument| argument.arity == ArgumentArity::Required)
        .count();
    let argument_names: Vec<_> = (0..operation.arguments.len())
        .map(|index| format_ident!("arg_{index}"))
        .collect();
    let arguments = operation
        .arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| operation_argument(index, argument, &argument_names[index]));
    let arguments_call = if operation.takes_this {
        quote! {
                ctx.clone(),
                host::this_object(params)?,
                #(#argument_names),*
        }
    } else {
        quote! { ctx.clone(), #(#argument_names),* }
    };
    let call = method_call(interface, method, &arguments_call);
    let body = match operation.result {
        OperationResult::Object => quote! { #call },
        OperationResult::Undefined => quote! {
            #call?;
            Ok(Value::new_undefined(ctx.clone()))
        },
        OperationResult::Sequence => quote! {
            let result: Vec<Value> = #call?;
            host::sequence(&ctx, result)
        },
        OperationResult::StringSequence => quote! {
            let result: Vec<String> = #call?;
            let values = result
                .into_iter()
                .map(|item| rquickjs::String::from_str(ctx.clone(), &item).map(rquickjs::String::into_value))
                .collect::<Result<Vec<Value>>>()?;
            host::sequence(&ctx, values)
        },
        OperationResult::String => quote! {
            #call.map(rquickjs::String::into_value)
        },
        OperationResult::NullableString => quote! {
            #call.map(|value| value.map_or_else(|| Value::new_null(ctx.clone()), rquickjs::String::into_value))
        },
        OperationResult::Boolean => quote! {
            let result: bool = #call?;
            rquickjs::IntoJs::into_js(result, &ctx)
        },
        OperationResult::UnsignedShort => quote! {
            let result: u16 = #call?;
            rquickjs::IntoJs::into_js(result, &ctx)
        },
        OperationResult::Long => quote! {
            let result: i32 = #call?;
            rquickjs::IntoJs::into_js(result, &ctx)
        },
    };
    let body = if operation.reactions {
        quote! { crate::js::reactions::with_reactions(&ctx, || { #body }) }
    } else {
        body
    };
    quote! {
        #id => {
            host::require_arguments(params, #required)?;
            #(#arguments)*
            #body
        }
    }
}

/// `[RustFromJs=PATH]`: convert the raw argument with `PATH::from_js`, keeping
/// the renderer's exact code-unit and pristine-string argument types.
fn from_js_argument(
    index: &TokenStream,
    variable: &proc_macro2::Ident,
    path: &syn::Path,
) -> TokenStream {
    quote! {
        let value = params.arg(#index).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        let #variable: super::#path = rquickjs::FromJs::from_js(&ctx, value)?;
    }
}

fn operation_argument(
    index: usize,
    argument: &OperationArgument,
    variable: &proc_macro2::Ident,
) -> TokenStream {
    if argument.arity == ArgumentArity::Variadic {
        if matches!(&argument.type_, ReturnType::Value) && argument.from_js.is_none() {
            // `[RustValue]` variadics hand the platform method the raw rest
            // arguments, matching the `(Node or DOMString)...` signatures.
            return quote! {
                // https://webidl.spec.whatwg.org/#es-overloads
                let mut values = Vec::with_capacity(params.len().saturating_sub(#index));
                for index in #index..params.len() {
                    values.push(
                        params.arg(index)
                            .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                    );
                }
                let #variable = rquickjs::function::Rest(values);
            };
        }
        let conversion =
            operation_argument_at(&quote! { index }, argument, &format_ident!("converted"));
        return quote! {
            // https://webidl.spec.whatwg.org/#es-overloads
            let mut #variable = Vec::with_capacity(params.len().saturating_sub(#index));
            for index in #index..params.len() {
                #conversion
                #variable.push(converted);
            }
        };
    }
    operation_argument_at(&quote! { #index }, argument, variable)
}

fn operation_argument_at(
    index: &TokenStream,
    argument: &OperationArgument,
    variable: &proc_macro2::Ident,
) -> TokenStream {
    let fetch = quote! {
        let value = params.arg(#index).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    };
    if let Some(path) = &argument.from_js {
        // https://webidl.spec.whatwg.org/#es-type-mapping
        return from_js_argument(index, variable, path);
    }
    match &argument.type_ {
        ReturnType::Node => quote! {
            #fetch
            let #variable = host::node_argument(&ctx, &value)?;
        },
        ReturnType::NullableNode => quote! {
            #fetch
            let #variable = host::nullable_node_argument(&ctx, &value)?;
        },
        ReturnType::Callback => quote! {
            #fetch
            let #variable = host::callback_argument(&ctx, &value)?;
        },
        ReturnType::Dictionary(name) => {
            let struct_name = format_ident!("{name}");
            quote! {
                #fetch
                let #variable = #struct_name::from_object(&ctx, &value)?;
            }
        }
        ReturnType::Enumeration(name) | ReturnType::Union(name, _) => {
            let name = format_ident!("{name}");
            quote! {
                #fetch
                let #variable = #name::from_value(&ctx, value)?;
            }
        }
        ReturnType::String => {
            let conversion = if argument.legacy_null_to_empty {
                quote! { host::legacy_null_string_argument(params, #index)? }
            } else {
                quote! { host::string_argument(params, #index, None)? }
            };
            if argument.arity == ArgumentArity::Optional {
                quote! {
                    #fetch
                    let #variable = if value.is_undefined() {
                        None
                    } else {
                        Some(#conversion)
                    };
                }
            } else {
                quote! { let #variable = #conversion; }
            }
        }
        ReturnType::NullableString => quote! {
            let #variable = host::nullable_string_argument(params, #index)?;
        },
        ReturnType::NullableDocumentType => {
            let fetch = if argument.null_default {
                quote! {
                    let value = params.arg(#index).unwrap_or_else(|| Value::new_null(ctx.clone()));
                }
            } else {
                fetch
            };
            quote! {
                #fetch
                let #variable = host::document_type_argument(&ctx, &value)?;
            }
        }
        ReturnType::Boolean
            if argument.arity == ArgumentArity::Optional && argument.boolean_default.is_none() =>
        {
            quote! {
                #fetch
                // https://webidl.spec.whatwg.org/#es-boolean
                let #variable = if value.is_undefined() {
                    None
                } else {
                    Some(host::boolean_argument(params, #index)?)
                };
            }
        }
        ReturnType::Boolean => boolean_argument(argument, variable, &fetch),
        ReturnType::UnsignedLong => unsigned_long_argument(variable, &fetch),
        ReturnType::Long => long_argument(variable, &fetch),
        ReturnType::Value => value_argument(index, argument, variable, &fetch),
        ReturnType::Double => quote! {
            #fetch
            // https://webidl.spec.whatwg.org/#es-double
            let converted: rquickjs::Coerced<f64> = rquickjs::FromJs::from_js(&ctx, value)?;
            let #variable = converted.0;
        },
        _ => unreachable!("validated operation argument"),
    }
}

fn unsigned_long_argument(variable: &proc_macro2::Ident, fetch: &TokenStream) -> TokenStream {
    quote! {
        #fetch
        // https://webidl.spec.whatwg.org/#es-unsigned-long
        let converted: rquickjs::Coerced<i32> = rquickjs::FromJs::from_js(&ctx, value)?;
        let #variable = converted.0.cast_unsigned();
    }
}

fn long_argument(variable: &proc_macro2::Ident, fetch: &TokenStream) -> TokenStream {
    quote! {
        #fetch
        // https://webidl.spec.whatwg.org/#es-long
        let converted: rquickjs::Coerced<i32> = rquickjs::FromJs::from_js(&ctx, value)?;
        let #variable = converted.0;
    }
}

fn value_argument(
    index: &TokenStream,
    argument: &OperationArgument,
    variable: &proc_macro2::Ident,
    fetch: &TokenStream,
) -> TokenStream {
    if argument.null_default {
        quote! {
            let value = params.arg(#index).unwrap_or_else(|| Value::new_null(ctx.clone()));
            let value = if value.is_undefined() {
                Value::new_null(ctx.clone())
            } else {
                value
            };
            let #variable = value;
        }
    } else {
        quote! {
            #fetch
            let #variable = value;
        }
    }
}

/// `boolean` argument conversion with its optional default
/// (<https://webidl.spec.whatwg.org/#es-boolean>).
fn boolean_argument(
    argument: &OperationArgument,
    variable: &proc_macro2::Ident,
    fetch: &TokenStream,
) -> TokenStream {
    let default = argument.boolean_default.map_or_else(
        || quote! {},
        |default| {
            quote! {
                let value = if value.is_undefined() {
                    Value::new_bool(ctx.clone(), #default)
                } else { value };
            }
        },
    );
    quote! {
        #fetch
        #default
        let converted: rquickjs::Coerced<bool> = rquickjs::FromJs::from_js(&ctx, value)?;
        let #variable = converted.0;
    }
}

fn constructor(interface: &Interface) -> (usize, TokenStream) {
    let Some(constructor) = &interface.constructor else {
        return (
            0,
            quote! { return Err(rquickjs::Exception::throw_type(&ctx, "Illegal constructor")); },
        );
    };
    let rust = &interface.rust;
    let payload = if interface.has_lifetime {
        quote! { #rust<'js> }
    } else {
        quote! { #rust }
    };
    let create = &constructor.rust;
    let construct = if interface.contract.is_some() {
        let contract = format_ident!("{}", interface.name);
        quote! { <#payload as #contract<'js>>::#create }
    } else {
        quote! { #rust::#create }
    };
    let required = constructor
        .arguments
        .iter()
        .take_while(|argument| argument.kind.is_required())
        .count();
    let argument_names: Vec<_> = (0..constructor.arguments.len())
        .map(|index| format_ident!("arg_{index}"))
        .collect();
    let arguments = constructor
        .arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let variable = &argument_names[index];
            match &argument.kind {
                ConstructorArgumentKind::String { default } => {
                    let default = if let Some(default) = default {
                        quote! { Some(#default) }
                    } else {
                        quote! { None }
                    };
                    quote! { let #variable = host::string_argument(params, #index, #default)?; }
                }
                ConstructorArgumentKind::Callback => quote! {
                    let value = params.arg(#index).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                    let #variable = host::callback_argument(&ctx, &value)?;
                },
                ConstructorArgumentKind::Dictionary(name) => {
                    // https://webidl.spec.whatwg.org/#es-dictionary
                    let struct_name = format_ident!("{name}");
                    quote! {
                        let value = params.arg(#index).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                        let #variable = #struct_name::from_object(&ctx, &value)?;
                    }
                }
            }
        });
    (
        required,
        quote! {
            host::require_constructor(params)?;
            host::require_arguments(params, #required)?;
            #(#arguments)*
            let prototype = host::constructor_prototype::<#payload>(params)?;
            let result = #construct(&ctx, #(#argument_names),*)?;
            return rquickjs::Class::instance_proto(result, prototype).map(rquickjs::Class::into_value);
        },
    )
}

fn dictionary(dictionary: &Dictionary) -> TokenStream {
    let name = format_ident!("{}", dictionary.name);
    let defaults = dictionary.fields.iter().map(|field| {
        let rust = &field.rust;
        match &field.type_ {
            DictionaryFieldType::Boolean {
                default: Some(default),
            } => quote! { #rust: #default },
            DictionaryFieldType::Boolean { default: None }
            | DictionaryFieldType::StringSequence => {
                quote! { #rust: None }
            }
        }
    });
    let fields = dictionary.fields.iter().map(|field| {
        let rust = &field.rust;
        match &field.type_ {
            DictionaryFieldType::Boolean { default: Some(_) } => {
                quote! { pub(crate) #rust: bool }
            }
            DictionaryFieldType::Boolean { default: None } => {
                quote! { pub(crate) #rust: Option<bool> }
            }
            DictionaryFieldType::StringSequence => {
                quote! { pub(crate) #rust: Option<Vec<Vec<u16>>> }
            }
        }
    });
    let conversions = dictionary.fields.iter().map(|field| {
        let rust = &field.rust;
        let key = &field.name;
        match &field.type_ {
            DictionaryFieldType::Boolean {
                default: Some(default),
            } => quote! {
                #rust: host::dict_flag(ctx, &object, #key)?.unwrap_or(#default)
            },
            DictionaryFieldType::Boolean { default: None } => quote! {
                #rust: host::dict_flag(ctx, &object, #key)?
            },
            DictionaryFieldType::StringSequence => quote! {
                #rust: host::dict_string_sequence(ctx, &object, #key)?
            },
        }
    });
    quote! {
        // https://webidl.spec.whatwg.org/#es-dictionary
        // A dictionary carries every IDL member; the algorithm that receives it
        // may consume a subset, so unconsumed fields are not dead code.
        #[allow(dead_code, reason = "generated from IDL; algorithms may read a subset")]
        pub(crate) struct #name {
            #(#fields),*
        }
        impl #name {
            pub(crate) fn from_object<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Self> {
                if value.is_null() || value.is_undefined() {
                    return Ok(Self { #(#defaults),* });
                }
                let object: Object = value.clone().into_object().ok_or_else(|| {
                    rquickjs::Exception::throw_type(ctx, "dictionary must be an object")
                })?;
                Ok(Self {
                    #(#conversions),*
                })
            }
        }
    }
}

fn enumeration(enumeration: &Enumeration) -> TokenStream {
    let name = format_ident!("{}", enumeration.name);
    let variants = enumeration.values.iter().map(|(_, variant)| variant);
    let conversions = enumeration
        .values
        .iter()
        .map(|(text, variant)| quote! { #text => Ok(Self::#variant) });
    let strings = enumeration
        .values
        .iter()
        .map(|(text, variant)| quote! { Self::#variant => #text });
    quote! {
        #[derive(Clone, Copy)]
        pub(crate) enum #name { #(#variants),* }

        impl #name {
            // https://webidl.spec.whatwg.org/#es-enumeration
            fn from_value<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
                let string: rquickjs::Coerced<rquickjs::String> = rquickjs::FromJs::from_js(ctx, value)?;
                let string = match string.0.to_string() {
                    Ok(string) => string,
                    Err(rquickjs::Error::Utf8(_)) => return Err(rquickjs::Exception::throw_type(ctx, "invalid enumeration value")),
                    Err(error) => return Err(error),
                };
                match string.as_str() {
                    #(#conversions,)*
                    _ => Err(rquickjs::Exception::throw_type(ctx, "invalid enumeration value")),
                }
            }

            pub(crate) fn as_str(self) -> &'static str {
                match self { #(#strings),* }
            }
        }
    }
}

/// `[Unscopable]` member names for the `@@unscopables` object. Attributes
/// cannot be unscopable yet; the contract path rejects them at lowering.
fn unscopables(interface: &Interface) -> TokenStream {
    let names: Vec<&str> = interface
        .operations
        .iter()
        .filter(|operation| operation.unscopable)
        .map(|operation| operation.name.as_str())
        .collect();
    quote! {
        const UNSCOPABLES: &[&str] = &[#(#names),*];
    }
}

/// One generated module per interface, named after it, so a bindings file
/// that includes several interfaces cannot collide.
fn module_name(name: &str) -> proc_macro2::Ident {
    let snake = crate::names::snake_case(name);
    format_ident!("{snake}_generated")
}

/// A generated union enum with an ordered `from_value`, following the
/// `WebIDL` union conversion: platform-object members precede string
/// coercion (<https://webidl.spec.whatwg.org/#es-union>). Chromium
/// generates one class per flattened member set with the same trial order
/// (`third_party/blink/renderer/bindings/scripts/bind_gen/union.py`);
/// Firefox generates the same order in its union `Init`
/// (`dom/bindings/Codegen.py::getJSToNativeConversionInfo`). V8 and
/// `SpiderMonkey` glue does not transfer; only the trial order does.
fn union(union: &Union) -> TokenStream {
    let name = format_ident!("{}", union.name);
    let variants = union.members.iter().map(|member| {
        let variant = &member.variant;
        let type_ = match member.type_ {
            UnionMemberType::Node => quote! { host::NodeReference },
            UnionMemberType::String => quote! { rquickjs::String<'js> },
        };
        quote! { #variant(#type_) }
    });
    let mut trials = Vec::new();
    // Trial order follows the WebIDL union algorithm, not the declaration
    // order: platform objects precede string coercion. A Node instance must
    // match the interface member even when the union declares the string
    // first, while every other value stringifies.
    let mut ordered: Vec<_> = union.members.iter().collect();
    ordered.sort_by_key(|member| match member.type_ {
        UnionMemberType::Node => 0,
        UnionMemberType::String => 1,
    });
    for member in ordered {
        let variant = &member.variant;
        match member.type_ {
            UnionMemberType::Node => trials.push(quote! {
                // A Node is a platform object implementing the interface;
                // anything else falls through to string coercion. The probe
                // is strict: `Attr` has its own payload and stringifies.
                if host::is_node(ctx, &value) {
                    return host::node_argument(ctx, &value).map(Self::#variant);
                }
            }),
            UnionMemberType::String => trials.push(quote! {
                // https://webidl.spec.whatwg.org/#es-DOMString
                // Coerced rejects Symbol and stringifies everything else.
                rquickjs::FromJs::from_js(ctx, value).map(|string: rquickjs::Coerced<rquickjs::String>| Self::#variant(string.0))
            }),
        }
    }
    // String coercion returns or throws, so it ends the trial chain; a
    // trailing mismatch error only exists without a string member.
    let mismatch = if union
        .members
        .iter()
        .any(|member| matches!(member.type_, UnionMemberType::String))
    {
        quote! {}
    } else {
        quote! { Err(rquickjs::Exception::throw_type(ctx, "value does not match the union")) }
    };
    quote! {
        pub(crate) enum #name<'js> {
            #(#variants),*
        }

        impl<'js> #name<'js> {
            fn from_value(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
                #(#trials)*
                #mismatch
            }
        }
    }
}

fn legacy_codes(interface: &Interface) -> TokenStream {
    let names: Vec<_> = interface
        .constants
        .iter()
        .filter_map(|constant| {
            let name = constant.legacy_name.as_ref()?;
            let value = constant.value;
            Some(quote! { #name => #value, })
        })
        .collect();
    if names.is_empty() {
        quote! {}
    } else {
        quote! {
            // https://webidl.spec.whatwg.org/#dom-domexception-code
            pub(super) fn legacy_code(name: &rquickjs::String<'_>) -> rquickjs::Result<u16> {
                let name = match name.to_string() {
                    Ok(name) => name,
                    Err(rquickjs::Error::Utf8(_)) => return Ok(0),
                    Err(error) => return Err(error),
                };
                Ok(match name.as_str() { #(#names)* _ => 0 })
            }
        }
    }
}
