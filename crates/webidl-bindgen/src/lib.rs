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

    #[test]
    fn supported_fixtures_emit_complete_rust_modules() {
        for source in [
            INPUT,
            "[Exposed=Window, Rust=Payload] interface Empty { [Rust=create] constructor(); };",
            PARTIAL,
        ] {
            let output = compile(source).expect("compile supported fixture");
            let file = syn::parse_file(&output).expect("generated module is valid Rust syntax");
            let [syn::Item::Mod(module)] = file.items.as_slice() else {
                panic!("expected one generated module");
            };
            assert_eq!(module.ident, "webidl_generated");
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
            INPUT.replace("DOMString label;", "object label;"),
            INPUT.replace("Rust=Payload", "Rust=\"not a rust path\""),
            INPUT.replace("interface Sample", "interface Sample : Parent"),
            PARTIAL.replace("Node node", "optional Node node"),
            PARTIAL.replace("Node node", "Node... node"),
            PARTIAL.replace("Node insert", "static Node insert"),
            PARTIAL.replace("Node insert", "DOMString insert"),
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
