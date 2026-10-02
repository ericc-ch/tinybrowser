use quote::ToTokens;
use webidl_bindgen::{Source, compile_contracts, compile_javascript, validate_sources};

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
        text: "impl<'js> sample_generated::Sample<'js> for Payload { fn typo(&self, ctx: Ctx<'js>) {} }",
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
        text: "interface Element {}; [Exposed=Window, LegacyUnenumerableNamedProperties] interface HTMLCollection { readonly attribute unsigned long length; getter Element? item(unsigned long index); getter Element? namedItem(DOMString name); };",
    }];
    let indexed_named_rust = [Source {
        name: "html_collection.rs",
        text: "impl<'js> html_collection_generated::HTMLCollection<'js> for Payload { fn get_length(&self, ctx: &Ctx<'js>) -> Result<usize> { l(ctx) } fn item(&self, ctx: Ctx<'js>, arg_0: u32) -> Result<Value<'js>> { i(ctx) } fn named_item(&self, ctx: Ctx<'js>, arg_0: rquickjs::String<'js>) -> Result<Value<'js>> { n(ctx) } fn supported_names(&self, ctx: &Ctx<'js>) -> Result<Vec<String>> { s(ctx) } }",
    }];
    let bindings = compile_contracts(&indexed_named, &indexed_named_rust).expect("compile fixture");
    assert!(bindings[0].rust.contains("supported_names"));
}

#[test]
fn nullable_strings_and_platform_attributes_lower() {
    let idl = [Source {
        name: "mutation_record.idl",
        text: r"
            [Exposed=Window] interface MutationRecord {
                readonly attribute DOMString type;
                [SameObject] readonly attribute Node target;
                readonly attribute Node? previousSibling;
                readonly attribute DOMString? attributeName;
            };
        ",
    }];
    let rust = [Source {
        name: "mutation_record.rs",
        text: r"
            impl<'js> mutation_record_generated::MutationRecord<'js> for JsMutationRecord<'js> {
                fn get_type(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> { t(ctx) }
                fn get_target(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> { g(ctx) }
                fn get_previous_sibling(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> { p(ctx) }
                fn get_attribute_name(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> { a(ctx) }
            }
        ",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile fixture");
    let trait_text = generated_trait(&bindings[0].rust)
        .replace(' ', "")
        .replace(",)", ")");
    for fragment in [
        "get_target(&self,ctx:&Ctx<'js>)->Result<Value<'js>>",
        "get_attribute_name(&self,ctx:&Ctx<'js>)->Result<Option<rquickjs::String<'js>>>",
    ] {
        assert!(
            trait_text.contains(fragment),
            "missing {fragment}: {trait_text}"
        );
    }
}

#[test]
fn callbacks_dictionaries_and_sequences_lower_with_receiver() {
    let idl = [Source {
        name: "mutation_observer.idl",
        text: r"
            interface MutationRecord {};
            callback MutationCallback = undefined (sequence<MutationRecord> records, MutationObserver observer);
            dictionary BaseOptions {
                boolean z = false;
                boolean a;
            };
            dictionary MutationObserverInit : BaseOptions {
                sequence<DOMString> attributeFilter;
                boolean attributes;
            };
            partial dictionary MutationObserverInit { boolean childList = false; };
            [Exposed=Window] interface MutationObserver {
                constructor(MutationCallback callback);
                undefined observe(Node target, optional MutationObserverInit options = {});
                sequence<MutationRecord> takeRecords();
            };
        ",
    }];
    let rust = [Source {
        name: "mutation_observer.rs",
        text: r"
            impl<'js> mutation_observer_generated::MutationObserver<'js> for JsMutationObserver {
                fn constructor(ctx: &Ctx<'js>, arg_0: rquickjs::Function<'js>) -> Result<Self> { c(ctx) }
                fn observe(&self, ctx: Ctx<'js>, this: Object<'js>, arg_0: host::NodeReference, arg_1: mutation_observer_generated::MutationObserverInit) -> Result<()> { o(ctx) }
                fn take_records(&self, ctx: Ctx<'js>) -> Result<Vec<Value<'js>>> { t(ctx) }
            }
        ",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile fixture");
    let syntax = syn::parse_file(&bindings[0].rust).expect("generated syntax");
    let [syn::Item::Mod(module)] = syntax.items.as_slice() else {
        panic!("expected a generated module");
    };
    let (_, items) = module.content.as_ref().expect("module body");
    let dictionary = items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(item) if item.ident == "MutationObserverInit" => Some(item),
            _ => None,
        })
        .expect("generated dictionary");
    assert_eq!(
        dictionary
            .fields
            .iter()
            .map(|field| field.ident.as_ref().expect("named field").to_string())
            .collect::<Vec<_>>(),
        ["a", "z", "attribute_filter", "attributes", "child_list"]
    );
    let source = bindings[0].rust.replace(' ', "");
    for fragment in [
        "structMutationObserverInit",
        "host::callback_argument",
        "this_object",
        "host::sequence",
    ] {
        assert!(
            source.contains(fragment),
            "missing {fragment}: {}",
            bindings[0].rust
        );
    }
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

#[test]
fn enum_attributes_lower_to_the_generated_enum() {
    let idl = [Source {
        name: "dom.idl",
        text: "enum Mode { \"open\", \"closed\" }; [Exposed=Window] interface Shadow { readonly attribute Mode mode; };",
    }];
    let rust = [Source {
        name: "shadow.rs",
        text: "impl<'js> shadow_generated::Shadow<'js> for Payload { fn get_mode(&self, ctx: &Ctx<'js>) -> Result<shadow_generated::Mode> { Ok(shadow_generated::Mode::Open) } }",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile enum attribute");
    let trait_text = generated_trait(&bindings[0].rust).replace(' ', "");
    assert!(
        trait_text.contains("fnget_mode(&self,ctx:&Ctx<'js>)->Result<Mode>"),
        "{trait_text}"
    );
    let source = bindings[0].rust.replace(' ', "");
    assert!(source.contains("enumMode"), "missing generated enum");
    assert!(source.contains("as_str"), "missing enum string conversion");
}

#[test]
fn node_or_string_unions_convert_interfaces_before_strings() {
    let idl = [Source {
        name: "dom.idl",
        text: "[Exposed=Window] interface Sample { undefined append((Node or DOMString)... nodes); };",
    }];
    let rust = [Source {
        name: "sample.rs",
        text: "impl<'js> sample_generated::Sample<'js> for Payload { fn append(&self, ctx: Ctx<'js>, nodes: Vec<sample_generated::DOMStringOrNode>) -> Result<()> { Ok(()) } }",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile union");
    let source = bindings[0].rust.replace(' ', "");
    for fragment in [
        "enumDOMStringOrNode",
        "Node(host::NodeReference)",
        "DOMString(rquickjs::String<'js>)",
        "node_argument",
        "Coerced<rquickjs::String>",
    ] {
        assert!(
            source.contains(&fragment.replace(' ', "")),
            "missing {fragment}"
        );
    }
    let node_trial = source.find("node_argument").expect("node trial");
    let string_trial = source.find("Coerced<rquickjs::String>").expect("string trial");
    assert!(
        node_trial < string_trial,
        "interface trial must precede string coercion"
    );
}

#[test]
fn unsupported_unions_fail_the_build() {
    let rust = [Source {
        name: "sample.rs",
        text: "impl<'js> sample_generated::Sample<'js> for Payload { fn append(&self, ctx: Ctx<'js>) -> Result<()> { Ok(()) } }",
    }];
    for text in [
        "[Exposed=Window] interface Sample { undefined append((Node or long)... nodes); };",
        "[Exposed=Window] interface Sample { undefined append((Node or DOMString)?... nodes); };",
        "[Exposed=Window] interface Sample { undefined append(([LegacyNullToEmptyString] DOMString or Node)... nodes); };",
    ] {
        let idl = [Source {
            name: "fixture.idl",
            text,
        }];
        assert!(
            compile_contracts(&idl, &rust).is_err(),
            "accepted {text}"
        );
    }
}

#[test]
fn unscopable_operations_list_in_unscopables() {
    let idl = [Source {
        name: "dom.idl",
        text: "[Exposed=Window] interface Sample { [Unscopable] undefined append(Node node); undefined keep(Node node); };",
    }];
    let rust = [Source {
        name: "sample.rs",
        text: "impl<'js> sample_generated::Sample<'js> for Payload { fn append(&self, ctx: Ctx<'js>, node: host::NodeReference) -> Result<()> { Ok(()) } fn keep(&self, ctx: Ctx<'js>, node: host::NodeReference) -> Result<()> { Ok(()) } }",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile unscopable");
    let source = bindings[0].rust.replace(' ', "");
    assert!(
        source.contains("UNSCOPABLES:&[&str]=&[\"append\"]"),
        "missing unscopables table"
    );
    assert!(source.contains("install_unscopables"), "missing install call");
}

#[test]
fn mixin_implementations_install_on_includers() {
    let idl = [Source {
        name: "dom.idl",
        text: "[Exposed=Window] interface Element {}; [Exposed=Window] interface Document {}; interface mixin Nodes { undefined append((Node or DOMString)... nodes); }; Element includes Nodes; Document includes Nodes;",
    }];
    let rust = [Source {
        name: "nodes.rs",
        text: "#[rquickjs::class] struct Payload; impl<'js> nodes_generated::Nodes<'js> for Payload { fn append(&self, ctx: Ctx<'js>, nodes: Vec<nodes_generated::DOMStringOrNode>) -> Result<()> { Ok(()) } }",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile mixin");
    assert_eq!(bindings[0].interface, "Nodes");
    let source = bindings[0].rust.replace(' ', "");
    for target in ["Element", "Document"] {
        assert!(
            source.contains(&format!("ctx.globals().get(\"{target}\")")),
            "missing install target {target}"
        );
    }
    assert!(
        !source.contains("ctx.globals().get(\"Nodes\")"),
        "mixin must not install on itself"
    );
}

#[test]
fn reflect_attributes_install_without_implementation() {
    let idl = [Source {
        name: "html.idl",
        text: "[Exposed=Window] interface Group { [CEReactions, Reflect] attribute boolean disabled; [CEReactions, Reflect] attribute DOMString label; };",
    }];
    let rust = [Source {
        name: "group.rs",
        text: "#[rquickjs::class] struct Payload; impl<'js> group_generated::Group<'js> for Payload {}",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile reflect");
    let source = bindings[0].rust.replace(' ', "");
    for fragment in [
        "reflect_bool",
        "reflect_string",
        "reflect_set_bool",
        "\"disabled\"",
        "\"label\"",
    ] {
        assert!(
            source.contains(&fragment.replace(' ', "")),
            "missing {fragment}"
        );
    }
}

#[test]
fn reflect_conflicts_and_unsupported_shapes() {
    // An implementation method for a reflected name fails the build.
    let idl = [Source {
        name: "html.idl",
        text: "[Exposed=Window] interface Group { [Reflect] attribute boolean disabled; };",
    }];
    let rust = [Source {
        name: "group.rs",
        text: "#[rquickjs::class] struct Payload; impl<'js> group_generated::Group<'js> for Payload { fn get_disabled(&self, ctx: &Ctx<'js>) -> Result<bool> { Ok(true) } }",
    }];
    assert!(compile_contracts(&idl, &rust).is_err());
    // Unsupported reflect shapes without implementation methods are absent,
    // like any unimplemented member.
    for text in [
        "[Exposed=Window] interface Group { [Reflect] attribute unsigned long span; };",
        "[Exposed=Window] interface Group { [CEReactions, ReflectURL] attribute USVString src; };",
        "[Exposed=Window] interface Group { [Reflect] readonly attribute Element anchor; };",
    ] {
        let idl = [Source {
            name: "html.idl",
            text,
        }];
        let empty = [Source {
            name: "group.rs",
            text: "#[rquickjs::class] struct Payload; impl<'js> group_generated::Group<'js> for Payload {}",
        }];
        let bindings = compile_contracts(&idl, &empty).expect("unsupported reflect stays absent");
        assert!(
            !bindings[0].rust.contains("reflect_"),
            "unexpected reflection for {text}"
        );
    }
}

#[test]
fn reflect_explicit_content_name() {
    let idl = [Source {
        name: "html.idl",
        text: "[Exposed=Window] interface Group { [Reflect=\"for\"] attribute DOMString target; };",
    }];
    let rust = [Source {
        name: "group.rs",
        text: "#[rquickjs::class] struct Payload; impl<'js> group_generated::Group<'js> for Payload {}",
    }];
    let bindings = compile_contracts(&idl, &rust).expect("compile named reflect");
    assert!(
        bindings[0].rust.replace(' ', "").contains("\"for\""),
        "missing explicit content name"
    );
}

#[test]
fn unforgeable_and_scoped_native_members_fail_the_build() {
    let rust = [Source {
        name: "fixture.rs",
        text: "impl<'js> sample_generated::Sample<'js> for Payload { fn get_flag(&self, ctx: &Ctx<'js>) -> Result<bool> { Ok(true) } }",
    }];
    for text in [
        "[Exposed=Window] interface Sample { [LegacyUnforgeable] readonly attribute boolean flag; };",
        "[Exposed=Window] interface Sample {}; [SecureContext] partial interface Sample { readonly attribute boolean flag; };",
        "[Exposed=Window] interface Sample {}; [SecureContext] interface mixin Extra { readonly attribute boolean flag; }; Sample includes Extra;",
    ] {
        let idl = [Source {
            name: "fixture.idl",
            text,
        }];
        let error = compile_contracts(&idl, &rust)
            .err()
            .expect("scoped or unforgeable member must fail")
            .to_string();
        assert!(
            error.contains("not supported"),
            "unexpected diagnostic for {text}: {error}"
        );
    }
}

const JS_ENCODER_IDL: &str = "[Exposed=Window] interface TextEncoder { readonly attribute DOMString encoding; Uint8Array encode(optional DOMString input = \"\"); };";

#[test]
fn javascript_contracts_insert_safe_reads_and_realm_results() {
    let idl = [Source {
        name: "encoding.idl",
        text: JS_ENCODER_IDL,
    }];
    let source = Source {
        name: "fixture.js",
        text: "globalThis.TextEncoder = __tbInstallInterface(class TextEncoder { get encoding() { return 'utf-8'; } encode(input) { return new Uint8Array(); } });",
    };
    let output = compile_javascript(&idl, &source)
        .expect("compile JS contract")
        .source;
    for fragment in [
        "args.length > 0 && args[0] !== undefined",
        "__tbIDLRealmUint8Array(value, false, realm)",
    ] {
        assert!(output.contains(fragment), "missing {fragment}: {output}");
    }
}

#[test]
fn javascript_unsupported_implementations_fail_the_build() {
    let idl = [Source {
        name: "encoding.idl",
        text: JS_ENCODER_IDL,
    }];
    for text in [
        "__tbInstallInterface(class TextEncoder { get encoding() { return 'x'; } encode(input) { return new Uint8Array(); } extra() {} })",
        "__tbInstallInterface(class TextEncoder { get encoding() { return 'x'; } encode() { return new Uint8Array(); } })",
        "__tbInstallInterface(class TextEncoder { get encoding() { return 'x'; } }); __tbInstallInterface(class TextEncoder { get encoding() { return 'x'; } })",
        "__tbInstallInterface(function() {}, 'TextEncoder', null, [])",
    ] {
        let source = Source {
            name: "fixture.js",
            text,
        };
        assert!(
            compile_javascript(&idl, &source).is_err(),
            "accepted {text}"
        );
    }
}
