//! Emit tables and a single dispatch function per interface, not per member.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::model::{
    Attribute, ConstructorArgumentKind, Dictionary, DictionaryFieldType, Enumeration,
    GetterMapping, Interface, InterfaceKind, Operation, OperationArgument, OperationResult,
    PrototypeParent, ReturnType,
};

pub(crate) fn interface(interface: &Interface) -> TokenStream {
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
        .filter(|argument| !argument.optional)
        .count();
        quote! { host::Member { name: #name, kind: host::MemberKind::Method { operation: host::Operation::new(#id), length: #length } } }
    });
    let members = attributes_members.chain(operation_members);
    let (routes, groups) = dispatch_groups(interface, &payload, "dispatch");
    let (alternate_dispatch, alternate_groups) = alternate_dispatch(interface);
    let constants = interface.constants.iter().map(|constant| {
        let name = &constant.name;
        let value = constant.value;
        quote! { host::Constant { name: #name, value: #value } }
    });
    let legacy_code = legacy_codes(interface);
    let definition = definition(interface, &payload, required);
    let conversion_import = if interface.attributes.is_empty() {
        quote! {}
    } else {
        quote! { use rquickjs::IntoJs; }
    };
    let constructor_dispatch = match interface.kind {
        InterfaceKind::Complete => quote! {
            if operation.index() == 0 {
                #constructor_body
            }
        },
        InterfaceKind::Partial => quote! {},
    };
    let receiver_check = if interface.rust == "JsNode" && interface.name != "Node" {
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

            const MEMBERS: &[host::Member] = &[#(#members),*];
            const CONSTANTS: &[host::Constant] = &[#(#constants),*];
            #definition
            #(#dictionaries)*
            #(#enumerations)*

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
        arms.push((index * 2 + 1, false, getter_dispatch((index, attribute))));
        if let Some(setter) = setter_dispatch((index, attribute)) {
            arms.push((index * 2 + 2, true, setter));
        }
    }
    for (index, operation) in interface.operations.iter().enumerate() {
        let id = interface.attributes.len() * 2 + index + 1;
        arms.push((id, true, operation_dispatch(id, operation)));
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

                fn prototype(ctx: &Ctx<'js>) -> Result<Option<Object<'js>>> {
                    host::prototype(ctx, #name, #parent, MEMBERS, CONSTANTS, dispatch).map(Some)
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
        InterfaceKind::Partial => quote! {
            pub(crate) fn install(ctx: &Ctx<'_>) -> Result<()> {
                let constructor: Object = ctx.globals().get(#name)?;
                let prototype: Object = constructor.get("prototype")?;
                host::install_members(&prototype, MEMBERS, CONSTANTS, dispatch)
            }
        },
    }
}

fn getter_dispatch((index, getter): (usize, &Attribute)) -> TokenStream {
    let id = index * 2 + 1;
    let method = &getter.rust;
    let body = match getter.mapping {
        GetterMapping::Field => quote! { receiver.#method.clone().into_js(&ctx) },
        GetterMapping::Method => match getter.return_type {
            ReturnType::String => {
                quote! { let result = receiver.#method(&ctx)?; result.into_js(&ctx) }
            }
            ReturnType::UsvString => quote! {
                let result = receiver.#method(&ctx)?;
                let result = result.to_string_lossy();
                result.into_js(&ctx)
            },
            ReturnType::Boolean => quote! {
                let result: bool = receiver.#method(&ctx)?;
                result.into_js(&ctx)
            },
            ReturnType::UnsignedShort => {
                quote! { let result: u16 = receiver.#method(&ctx)?; result.into_js(&ctx) }
            }
            ReturnType::UnsignedLong => quote! {
                let result = receiver.#method(&ctx)?;
                result.into_js(&ctx)
            },
            ReturnType::NullableString => quote! {
                let result: Option<rquickjs::String> = receiver.#method(&ctx)?;
                match result {
                    Some(result) => result.into_js(&ctx),
                    None => Ok(Value::new_null(ctx)),
                }
            },
            ReturnType::Double => quote! {
                // https://webidl.spec.whatwg.org/#idl-DOMHighResTimeStamp
                let result: f64 = receiver.#method(&ctx)?;
                result.into_js(&ctx)
            },
            ReturnType::NullableNode | ReturnType::NodeList | ReturnType::PlatformObject => {
                quote! { receiver.#method(&ctx) }
            }
            ReturnType::Node
            | ReturnType::Callback
            | ReturnType::Dictionary(_)
            | ReturnType::Enumeration(_)
            | ReturnType::InterfaceSequence
            | ReturnType::Value
            | ReturnType::NullableDocumentType => {
                unreachable!("validated field mapping")
            }
        },
    };
    quote! { #id => { #body } }
}

fn setter_dispatch((index, attribute): (usize, &Attribute)) -> Option<TokenStream> {
    let method = attribute.setter.as_ref()?;
    let id = index * 2 + 2;
    let convert = match attribute.return_type {
        ReturnType::String if attribute.legacy_null_to_empty => quote! {
            host::legacy_null_string_argument(params, 0)?
        },
        ReturnType::String => quote! { host::string_argument(params, 0, None)? },
        ReturnType::NullableString => quote! { host::nullable_string_argument(params, 0)? },
        // https://webidl.spec.whatwg.org/#es-boolean
        ReturnType::Boolean => quote! { host::boolean_argument(params, 0)? },
        _ => unreachable!("validated string or boolean setter"),
    };
    let body = quote! {
        receiver.#method(&ctx, value)?;
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

fn operation_dispatch(id: usize, operation: &Operation) -> TokenStream {
    let method = &operation.rust;
    let required = operation
        .arguments
        .iter()
        .take_while(|argument| !argument.optional)
        .count();
    let argument_names: Vec<_> = (0..operation.arguments.len())
        .map(|index| format_ident!("arg_{index}"))
        .collect();
    let arguments = operation
        .arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| operation_argument(index, argument, &argument_names[index]));
    let call = if operation.takes_this {
        quote! {
            receiver.#method(
                ctx.clone(),
                host::this_object(params)?,
                #(#argument_names),*
            )
        }
    } else {
        quote! { receiver.#method(ctx.clone(), #(#argument_names),*) }
    };
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
        OperationResult::String => quote! {
            #call.map(rquickjs::String::into_value)
        },
        OperationResult::Boolean => quote! {
            let result: bool = #call?;
            rquickjs::IntoJs::into_js(result, &ctx)
        },
        OperationResult::UnsignedShort => quote! {
            let result: u16 = #call?;
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

fn operation_argument(
    index: usize,
    argument: &OperationArgument,
    variable: &proc_macro2::Ident,
) -> TokenStream {
    let fetch = quote! {
        let value = params.arg(#index).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    };
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
        ReturnType::Enumeration(name) => {
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
            if argument.optional {
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
        ReturnType::Boolean => {
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
        ReturnType::UnsignedLong => quote! {
            #fetch
            // https://webidl.spec.whatwg.org/#es-unsigned-long
            let converted: rquickjs::Coerced<i32> = rquickjs::FromJs::from_js(&ctx, value)?;
            let #variable = converted.0.cast_unsigned();
        },
        ReturnType::Value => quote! {
            #fetch
            let #variable = value;
        },
        _ => unreachable!("validated operation argument"),
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
            let result = #rust::#create(&ctx, #(#argument_names),*)?;
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

/// One generated module per interface, named after it, so a bindings file
/// that includes several interfaces cannot collide.
fn module_name(name: &str) -> proc_macro2::Ident {
    let mut snake = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (index, char) in chars.iter().enumerate() {
        if !char.is_ascii_uppercase() {
            snake.push(*char);
            continue;
        }
        // Word boundary on lower-to-upper (`mutation|Observer`) and acronym
        // end (`DOM|Exception`); never before the first character.
        let before_lower = index > 0 && chars[index - 1].is_ascii_lowercase();
        let after_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
        if index > 0 && (before_lower || after_lower) {
            snake.push('_');
        }
        snake.push(char.to_ascii_lowercase());
    }
    format_ident!("{snake}_generated")
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
