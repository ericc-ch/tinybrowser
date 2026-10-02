use quote::ToTokens;
use webidl_bindgen::{Source, compile_contracts, validate_sources};

fn generated_trait(rust: &str) -> String {
    let syntax = syn::parse_file(rust).expect("generated syntax");
    let [syn::Item::Mod(module)] = syntax.items.as_slice() else {
        panic!("expected a generated module");
    };
    let (_, items) = module.content.as_ref().expect("module body");
    let traits: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Trait(item) => Some(item.to_token_stream().to_string()),
            _ => None,
        })
        .collect();
    traits.join("\n")
}

#[test]
fn contracts_follow_implementations_and_resolved_declarations() {
    let idl = [Source {
        name: "fixture.idl",
        text: r#"
        typedef unsigned short Count;
        [Exposed=*] interface Sample {
            constructor(optional DOMString label = "fixture");
            readonly attribute DOMString label;
            readonly attribute DOMString unsupported;
        };
        partial interface Sample { readonly attribute Count count; };
        interface mixin Common { readonly attribute boolean enabled; };
        Sample includes Common;
    "#,
    }];
    let rust = [Source {
        name: "fixture.rs",
        text: r"
        impl<'js> sample_generated::Sample<'js> for Payload<'js> {
            fn constructor(ctx: &Ctx<'js>, label: String<'js>) -> Result<Self> { build(ctx, label) }
            fn get_label(&self, ctx: &Ctx<'js>) -> Result<String<'js>> { read(ctx) }
            fn get_count(&self, ctx: &Ctx<'js>) -> Result<u16> { count(ctx) }
            fn get_enabled(&self, ctx: &Ctx<'js>) -> Result<bool> { enabled(ctx) }
        }
    ",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile fixture");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].interface, "Sample");
    assert_eq!(generated_trait(&bindings[0].rust).replace(' ', ""), quote::quote! {
        pub(super) trait Sample<'js> {
            fn constructor(ctx: &Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Self> where Self: Sized;
            fn get_label(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>>;
            fn get_count(&self, ctx: &Ctx<'js>) -> Result<u16>;
            fn get_enabled(&self, ctx: &Ctx<'js>) -> Result<bool>;
        }
    }.to_string().replace(' ', ""));
}

#[test]
fn conflicting_providers_and_unknown_contract_methods_are_diagnostics() {
    let idl = [Source {
        name: "fixture.idl",
        text: "[Exposed=Window] interface Sample { readonly attribute DOMString label; };",
    }];
    let unknown = [Source {
        name: "fixture.rs",
        text: "impl<'js> sample_generated::Sample<'js> for Payload { fn typo() {} }",
    }];
    assert_eq!(
        compile_contracts(&idl, &unknown)
            .err()
            .expect("unknown method")
            .to_string(),
        "Sample: methods do not match a supported IDL contract: typo"
    );
    let duplicates = [Source {
        name: "fixture.rs",
        text: "impl<'js> sample_generated::Sample<'js> for A {} impl<'js> sample_generated::Sample<'js> for B {}",
    }];
    assert_eq!(
        compile_contracts(&idl, &duplicates)
            .err()
            .expect("duplicate provider")
            .to_string(),
        "fixture.rs: multiple native implementations of Sample are not supported"
    );
}

#[test]
fn getters_lower_to_property_hooks_and_operations() {
    let idl = [Source {
        name: "node_list.idl",
        text: r"
            [Exposed=Window] interface NodeList {
                getter Node? item(unsigned long index);
                readonly attribute unsigned long length;
                iterable<Node>;
            };
        ",
    }];
    let rust = [Source {
        name: "node_list.rs",
        text: r"
            impl<'js> node_list_generated::NodeList<'js> for Payload {
                fn get_length(&self, ctx: &Ctx<'js>) -> Result<usize> { length(ctx) }
                fn item(&self, ctx: Ctx<'js>, arg_0: u32) -> Result<Value<'js>> { item(ctx, arg_0) }
            }
        ",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile fixture");
    assert_eq!(bindings[0].interface, "NodeList");
    for fragment in ["exotic_get_own_property", "fn get_length", "fn item"] {
        assert!(
            bindings[0].rust.contains(fragment),
            "missing {fragment}: {}",
            bindings[0].rust
        );
    }
    let named_only = [Source {
        name: "named.idl",
        text: "[Exposed=Window] interface Named { getter Node? item(DOMString name); };",
    }];
    let named_rust = [Source {
        name: "named.rs",
        text: "impl<'js> named_generated::Named<'js> for Payload { fn item(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Value<'js>> { item(ctx) } }",
    }];
    assert!(compile_contracts(&named_only, &named_rust).is_err());

    let indexed_named = [Source {
        name: "html_collection.idl",
        text: "[Exposed=Window, LegacyUnenumerableNamedProperties] interface HTMLCollection { readonly attribute unsigned long length; getter Element? item(unsigned long index); getter Element? namedItem(DOMString name); };",
    }];
    let indexed_named_rust = [Source {
        name: "html_collection.rs",
        text: "impl<'js> html_collection_generated::HTMLCollection<'js> for Payload { fn get_length(&self, ctx: &Ctx<'js>) -> Result<usize> { l(ctx) } fn item(&self, ctx: Ctx<'js>, arg_0: u32) -> Result<Value<'js>> { i(ctx) } fn named_item(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Value<'js>> { n(ctx) } fn supported_names(&self, ctx: &Ctx<'js>) -> Result<Vec<String>> { s(ctx) } }",
    }];
    let bindings = compile_contracts(&indexed_named, &indexed_named_rust).expect("compile fixture");
    assert!(bindings[0].rust.contains("supported_names"));
}

#[test]
fn malformed_graphs_return_named_diagnostics() {
    for (text, diagnostic) in [
        (
            "partial interface Missing {};",
            "partial Missing has no complete definition",
        ),
        (
            "interface A : B {}; interface B : A {};",
            "inheritance cycle at A",
        ),
        (
            "interface A {}; A includes Missing;",
            "included Missing is not an interface mixin",
        ),
        (
            "interface A {}; dictionary A {};",
            "fixture.idl: duplicate definition A",
        ),
        (
            "interface A {}; partial dictionary A {};",
            "partial A has a different declaration kind",
        ),
    ] {
        assert_eq!(
            validate_sources(&[Source {
                name: "fixture.idl",
                text
            }])
            .expect_err("invalid graph")
            .to_string(),
            diagnostic
        );
    }
}

#[test]
fn implemented_unsupported_types_do_not_disappear() {
    let idl = [Source {
        name: "fixture.idl",
        text: "[Exposed=Window] interface Sample { readonly attribute object value; };",
    }];
    let rust = [Source {
        name: "fixture.rs",
        text: "impl<'js> sample_generated::Sample<'js> for Payload { fn get_value() {} }",
    }];
    assert!(compile_contracts(&idl, &rust).is_err());
}
