//! Emit tables and a single dispatch function per interface, not per member.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::model::{Getter, GetterMapping, Interface, InterfaceKind, Operation, ReturnType};

pub(crate) fn interface(interface: &Interface) -> TokenStream {
    let rust = &interface.rust;
    let payload = if interface.has_lifetime {
        quote! { #rust<'js> }
    } else {
        quote! { #rust }
    };
    let (required, constructor_body) = constructor(interface);
    let getters = interface.getters.iter().enumerate().map(getter_dispatch);
    let getters_members = interface.getters.iter().enumerate().map(|(index, getter)| {
        let id = index + 1;
        let name = &getter.name;
        quote! { host::Member { name: #name, kind: host::MemberKind::Getter(host::Operation::new(#id)) } }
    });
    let operation_members = interface.operations.iter().enumerate().map(|(index, operation)| {
        let id = interface.getters.len() + index + 1;
        let name = &operation.name;
        let length = operation.arguments.len();
        quote! { host::Member { name: #name, kind: host::MemberKind::Method { operation: host::Operation::new(#id), length: #length } } }
    });
    let members = getters_members.chain(operation_members);
    let operations = interface
        .operations
        .iter()
        .enumerate()
        .map(|(index, operation)| {
            let id = interface.getters.len() + index + 1;
            operation_dispatch(id, operation)
        });
    let constants = interface.constants.iter().map(|constant| {
        let name = &constant.name;
        let value = constant.value;
        quote! { host::Constant { name: #name, value: #value } }
    });
    let legacy_code = legacy_codes(interface);
    let definition = definition(interface, &payload, required);
    let conversion_import = if interface.getters.is_empty() {
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
        mod webidl_generated {
            use super::#rust;
            use crate::js::bindings::host;
            use rquickjs::{Ctx, Object, Result, Value};
            use rquickjs::function::Params;
            #conversion_import

            const MEMBERS: &[host::Member] = &[#(#members),*];
            const CONSTANTS: &[host::Constant] = &[#(#constants),*];
            #definition

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

fn getter_dispatch((index, getter): (usize, &Getter)) -> TokenStream {
    let id = index + 1;
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
            ReturnType::Node | ReturnType::NodeList => unreachable!("validated field mapping"),
        },
    };
    quote! { #id => { #body } }
}

fn operation_dispatch(id: usize, operation: &Operation) -> TokenStream {
    let method = &operation.rust;
    let required = operation.arguments.len();
    let argument_names: Vec<_> = (0..required)
        .map(|index| format_ident!("arg_{index}"))
        .collect();
    let arguments = operation
        .arguments
        .iter()
        .enumerate()
        .map(|(index, type_)| {
            let variable = &argument_names[index];
            let convert = match type_ {
                ReturnType::Node => quote! { crate::js::bindings::required_node(&ctx, &value)? },
                ReturnType::NullableNode => {
                    quote! { crate::js::bindings::optional_node(&ctx, &value)? }
                }
                _ => unreachable!("validated operation argument"),
            };
            quote! {
                let value = params.arg(#index).unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                let #variable = #convert;
            }
        });
    quote! {
        #id => {
            host::require_arguments(params, #required)?;
            #(#arguments)*
            receiver.#method(ctx.clone(), #(#argument_names),*)
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
        .take_while(|arg| arg.default.is_none())
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
            let default = if let Some(default) = &argument.default {
                quote! { Some(#default) }
            } else {
                quote! { None }
            };
            quote! { let #variable = host::string_argument(params, #index, #default)?; }
        });
    (
        required,
        quote! {
            host::require_constructor(params)?;
            host::require_arguments(params, #required)?;
            #(#arguments)*
            let prototype = host::constructor_prototype::<#payload>(params)?;
            let result = #rust::#create(#(#argument_names),*);
            return rquickjs::Class::instance_proto(result, prototype).map(rquickjs::Class::into_value);
        },
    )
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
