//! Emit tables and a single dispatch function per interface, not per member.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::model::{
    Attribute, ConstructorArgumentKind, Dictionary, DictionaryFieldType, GetterMapping, Interface,
    InterfaceKind, Operation, OperationResult, ReturnType,
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
    let getters = interface.attributes.iter().enumerate().map(getter_dispatch);
    let setters = interface
        .attributes
        .iter()
        .enumerate()
        .filter_map(setter_dispatch);
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
    let operations = interface
        .operations
        .iter()
        .enumerate()
        .map(|(index, operation)| {
            let id = interface.attributes.len() * 2 + index + 1;
            operation_dispatch(id, operation)
        });
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
    quote! {
        mod #module {
            use super::#rust;
            use crate::js::bindings::host;
            use rquickjs::{Ctx, Object, Result, Value};
            use rquickjs::function::Params;
            #conversion_import

            const MEMBERS: &[host::Member] = &[#(#members),*];
            const CONSTANTS: &[host::Constant] = &[#(#constants),*];
            #definition
            #(#dictionaries)*

            // https://webidl.spec.whatwg.org/#es-interface-call
            fn dispatch<'js>(operation: host::Operation, params: &Params<'_, 'js>) -> Result<Value<'js>> {
                let ctx = params.ctx().clone();
                #constructor_dispatch
                // https://webidl.spec.whatwg.org/#es-attributes
                // https://webidl.spec.whatwg.org/#es-operations
                let receiver = host::receiver::<#payload>(params)?;
                let receiver = receiver.borrow();
                match operation.index() {
                    #(#getters,)*
                    #(#setters,)*
                    #(#operations,)*
                    _ => Err(rquickjs::Exception::throw_internal(&ctx, "unknown native operation")),
                }
            }

            #legacy_code
        }
    }
}

fn definition(interface: &Interface, payload: &TokenStream, required: usize) -> TokenStream {
    let name = &interface.name;
    let parent = if let Some(parent) = &interface.parent_intrinsic {
        quote! { Some(#parent) }
    } else {
        quote! { None }
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
                    host::constructor(ctx, #name, #required, &prototype, CONSTANTS,
                        host::HostCall::new(dispatch, host::Operation::new(0))).map(Some)
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
                quote! { let result: rquickjs::String = receiver.#method(&ctx)?; result.into_js(&ctx) }
            }
            ReturnType::UnsignedShort => {
                quote! { let result: u16 = receiver.#method(&ctx)?; result.into_js(&ctx) }
            }
            ReturnType::NullableString => quote! {
                let result: Option<rquickjs::String> = receiver.#method(&ctx)?;
                match result {
                    Some(result) => result.into_js(&ctx),
                    None => Ok(Value::new_null(ctx)),
                }
            },
            ReturnType::NullableNode => quote! { receiver.#method(&ctx) },
            ReturnType::Node
            | ReturnType::NodeList
            | ReturnType::Callback
            | ReturnType::Dictionary(_)
            | ReturnType::RecordSequence => {
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
        ReturnType::String => quote! { host::string_argument(params, 0, None)? },
        ReturnType::NullableString => quote! { host::nullable_string_argument(params, 0)? },
        _ => unreachable!("validated string setter"),
    };
    Some(quote! {
        #id => {
            // https://webidl.spec.whatwg.org/#es-attributes
            // Chromium and Firefox reject an omitted setter argument;
            // WebIDL instead converts undefined, so follow that algorithm.
            let value = #convert;
            receiver.#method(&ctx, value)?;
            Ok(Value::new_undefined(ctx))
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
        .map(|(index, argument)| {
            let variable = &argument_names[index];
            let fetch = quote! {
                let value = params.arg(#index).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
            };
            match &argument.type_ {
                ReturnType::Node => quote! {
                    #fetch
                    let #variable = crate::js::bindings::required_node(&ctx, &value)?;
                },
                ReturnType::NullableNode => quote! {
                    #fetch
                    let #variable = crate::js::bindings::optional_node(&ctx, &value)?;
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
                _ => unreachable!("validated operation argument"),
            }
        });
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
        OperationResult::Node => quote! { #call },
        OperationResult::Undefined => quote! {
            #call?;
            Ok(Value::new_undefined(ctx))
        },
        OperationResult::RecordSequence => quote! {
            let result: Vec<Value> = #call?;
            host::sequence(&ctx, result)
        },
    };
    quote! {
        #id => {
            host::require_arguments(params, #required)?;
            #(#arguments)*
            #body
        }
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
