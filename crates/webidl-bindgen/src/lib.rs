//! Build-time compiler for tinybrowser's native Web IDL bindings.

mod emit;
mod model;

use std::fmt;

/// A declaration the binding compiler cannot represent safely.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Compile a single native interface into a Rust module.
///
/// Unsupported declarations are errors, never omitted members. The compiler's
/// supported subset grows with migrated interfaces rather than accepting IDL
/// whose semantics the runtime cannot yet implement.
///
/// # Errors
/// Returns a diagnostic for invalid syntax, unsupported semantics, or mappings.
pub fn compile(source: &str) -> Result<String, Error> {
    let interface = model::Interface::parse(source)?;
    let output = emit::interface(&interface);
    let syntax =
        syn::parse2(output).map_err(|error| Error(format!("invalid generated Rust: {error}")))?;
    Ok(prettyplease::unparse(&syntax))
}

#[cfg(test)]
mod tests {
    use super::compile;

    const INPUT: &str = r#"
        [Exposed=Window, Rust=Payload] interface Sample {
            [Rust=create] constructor(optional DOMString label = "");
            [Rust=label] readonly attribute DOMString label;
            const unsigned short LIMIT = 7;
        };
    "#;

    const PARTIAL: &str = r"
        [Exposed=Window, Rust=Payload] partial interface Sample {
            [Rust=insert] Node insert(Node node, Node? child);
        };
    ";

    const SETTERS: &str = r"
        [Exposed=Window, Rust=Payload] partial interface Sample {
            [Rust=label, RustSet=set_label] attribute DOMString label;
            [Rust=value, RustSet=set_value] attribute DOMString? value;
        };
    ";

    const VALUE_OPERATIONS: &str = r"
        [Exposed=Window, Rust=Payload] interface Serializer {
            [Rust=new] constructor();
            [Rust=serialize] DOMString serializeToString([RustValue] Node root);
            [Rust=parse] Document parse(DOMString source);
        };
    ";

    const CALLBACK: &str = r"
        callback MutationCallback = undefined (sequence<MutationRecord> records, MutationObserver observer);
        [Exposed=Window, Rust=Observer] interface MutationObserver {
            [Rust=create] constructor(MutationCallback callback);
        };
    ";

    const DICTIONARY: &str = r"
        dictionary ObserverInit {
            [Rust=child_list] boolean childList = false;
            [Rust=attributes] boolean attributes;
            [Rust=filter] sequence<DOMString> attributeFilter;
        };
        [Exposed=Window, Rust=Observer] interface MutationObserver {
            [Rust=observe] undefined observe(Node target, optional ObserverInit options = {});
            [Rust=records] sequence<MutationRecord> takeRecords();
        };
    ";

    #[test]
    fn supported_fixtures_emit_complete_rust_modules() {
        for (source, module) in [
            (INPUT, "sample_generated"),
            (
                "[Exposed=Window, Rust=Payload] interface Empty { [Rust=create] constructor(); };",
                "empty_generated",
            ),
            (PARTIAL, "sample_generated"),
            (SETTERS, "sample_generated"),
            (VALUE_OPERATIONS, "serializer_generated"),
            (CALLBACK, "mutation_observer_generated"),
        ] {
            let output = compile(source).expect("compile supported fixture");
            let file = syn::parse_file(&output).expect("generated module is valid Rust syntax");
            let [syn::Item::Mod(item)] = file.items.as_slice() else {
                panic!("expected one generated module");
            };
            assert_eq!(item.ident, module);
        }
    }

    #[test]
    fn malformed_suffix_is_not_silently_dropped() {
        let source = format!("{INPUT} garbage");
        assert!(compile(&source).is_err());
    }

    #[test]
    fn unknown_semantics_fail_the_build() {
        for source in [
            INPUT.replace("Rust=label", "Rust=label, Replaceable"),
            INPUT.replace("readonly attribute", "attribute"),
            INPUT.replace("Rust=label", "Rust=label, RustSet=set_label"),
            SETTERS.replace("DOMString label", "unsigned short label"),
            INPUT.replace("DOMString label;", "object label;"),
            INPUT.replace("Rust=Payload", "Rust=\"not a rust path\""),
            INPUT
                .replace("interface Sample", "interface Sample : Parent")
                .replace("Rust=Payload", "Rust=Payload, RustPrototype=Error"),
            INPUT.replace("interface Sample", "interface Sample : Sample"),
            VALUE_OPERATIONS.replace("DOMString source", "Document source"),
            VALUE_OPERATIONS
                .replace("Rust=parse", "Rust=parse, NewObject")
                .replace("Document parse", "boolean parse"),
            VALUE_OPERATIONS
                .replace("Rust=parse", "Rust=parse, NewObject")
                .replace("Document parse", "Document? parse"),
            INPUT.replace("Rust=label", "Rust=label, CEReactions"),
            INPUT.replace("Rust=Payload", "Rust=Payload, RustAlternateLifetime"),
            INPUT.replace("Rust=label", "Rust=label, RustSetFromJs=Converted"),
            INPUT.replace("Rust=label", "Rust=label, RustValue, PutForwards=value"),
            INPUT
                .replace("DOMString label;", "DOMTokenList? label;")
                .replace("Rust=label", "Rust=label, RustValue, PutForwards=value"),
            PARTIAL.replace("Node node", "optional Node node"),
            PARTIAL.replace("Node node", "Node... node"),
            PARTIAL.replace("Node insert", "static Node insert"),
            PARTIAL.replace(
                "Node? child);",
                "Node? child); [Rust=other] Node insert(Node node);",
            ),
            PARTIAL.replace(
                "Node? child);",
                "Node? child); [Rust=create] constructor();",
            ),
            PARTIAL.replace(
                "Node? child);",
                "Node? child); const unsigned short LIMIT = 7;",
            ),
        ] {
            assert!(compile(&source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn callback_arguments_emit_callable_conversion() {
        let output = compile(CALLBACK).expect("compile callback fixture");
        assert!(
            output.contains("callback_argument"),
            "constructor must convert through the callback helper"
        );
    }

    #[test]
    fn dictionaries_emit_structs_and_optional_arguments() {
        let output = compile(DICTIONARY).expect("compile dictionary fixture");
        for fragment in [
            "struct ObserverInit",
            "from_object",
            "dict_flag",
            "dict_string_sequence",
        ] {
            assert!(
                output.contains(fragment),
                "dictionary output must contain {fragment}"
            );
        }
    }

    #[test]
    fn dictionary_semantics_fail_the_build() {
        for source in [
            DICTIONARY.replace(
                "[Rust=child_list] boolean childList",
                "[Rust=child_list] required boolean childList",
            ),
            DICTIONARY.replace("[Rust=attributes] ", ""),
            DICTIONARY.replace(
                "[Rust=filter] sequence<DOMString> attributeFilter;",
                "[Rust=filter] sequence<DOMString> attributeFilter;
                [Rust=again] boolean attributes;",
            ),
            DICTIONARY.replace("boolean attributes;", "DOMString attributes;"),
            DICTIONARY.replace("boolean attributes;", "boolean? attributes;"),
            DICTIONARY.replace(
                "sequence<DOMString> attributeFilter;",
                "sequence<DOMString> attributeFilter = [];",
            ),
            DICTIONARY.replace(
                "optional ObserverInit options = {}",
                "optional Node target = {}",
            ),
            DICTIONARY.replace(
                "(Node target, optional ObserverInit options = {})",
                "(optional ObserverInit options = {}, Node target)",
            ),
            DICTIONARY.replace("dictionary ObserverInit", "partial dictionary ObserverInit"),
            DICTIONARY.replace(
                "dictionary ObserverInit",
                "dictionary ObserverInit : Parent",
            ),
            DICTIONARY.replace(
                "[Rust=observe] undefined observe",
                "[Rust=observe] ObserverInit observe",
            ),
        ] {
            assert!(compile(&source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn callback_semantics_fail_the_build() {
        for source in [
            CALLBACK.replace(
                "callback MutationCallback = undefined",
                "callback MutationCallback = DOMString",
            ),
            format!("{CALLBACK}\ncallback MutationCallback = undefined ();"),
            CALLBACK.replace(
                "[Rust=create] constructor(MutationCallback callback);",
                "[Rust=create] constructor(optional MutationCallback callback);",
            ),
            CALLBACK.replace(
                "[Rust=create] constructor(MutationCallback callback);",
                "[Rust=watch] MutationCallback watch(MutationCallback callback);",
            ),
            CALLBACK.replace(
                "[Rust=create] constructor(MutationCallback callback);",
                "[Rust=create] constructor(MutationCallback callback);\n[Rust=hook] readonly attribute MutationCallback hook;",
            ),
            CALLBACK.replace(
                "callback MutationCallback = undefined",
                "dictionary MutationCallback { boolean flag; }; callback Unused = undefined",
            ),
            format!(
                "{CALLBACK}\n[Exposed=Window, Rust=Other] interface Other {{ [Rust=create] constructor(); }};"
            ),
        ] {
            assert!(compile(&source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn receiver_object_reaches_this_taking_operations() {
        const THIS: &str = r"
            [Exposed=Window, Rust=Payload] interface Sample {
                [Rust=create] constructor();
                [Rust=watch, RustThis] undefined watch(Node node);
            };
        ";
        let output = compile(THIS).expect("compile receiver fixture");
        assert!(
            output.contains("this_object"),
            "RustThis operations must receive the receiver object"
        );
    }

    #[test]
    fn mappings_and_duplicate_members_are_checked() {
        for source in [
            INPUT.replace("[Rust=label]", ""),
            INPUT.replace("const unsigned short LIMIT", "const unsigned short label"),
            INPUT.replace("Rust=label", "Rust=label, Rust=other"),
            INPUT.replace("[Rust=create]", "[Rust=create, RustFallible]"),
        ] {
            assert!(compile(&source).is_err(), "accepted {source}");
        }
    }
}
