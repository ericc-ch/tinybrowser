//! Build-time compiler for tinybrowser's native Web IDL bindings.

mod contracts;
mod database;
mod emit;
mod javascript;
mod model;
mod names;

/// A named input to the binding compiler. `name` is used in diagnostics and
/// `text` is parsed without changing declarations or adding annotations.
pub struct Source<'source> {
    /// File name used to identify an invalid input.
    pub name: &'source str,
    /// Complete UTF-8 source contents.
    pub text: &'source str,
}

/// Validate the structure of an unchanged IDL corpus across file boundaries.
///
/// Resolves partial declaration targets, mixin includes, and inheritance.
/// This does not install interfaces or claim platform conformance.
///
/// # Errors
/// Returns a named diagnostic for parse failures, duplicate declarations,
/// missing structural dependencies, incompatible declaration kinds, or cycles.
pub fn validate_sources(sources: &[Source<'_>]) -> Result<(), Error> {
    database::Database::parse(sources).map(|_| ())
}

/// One reflected enumerated attribute: the (interface, attribute) pair the
/// generated getter belongs to, the content attribute it reads, the
/// `Enumerated` type that owns the keyword set, and the two spec defaults.
///
/// The WPT IDL extracts carry the member but not its keyword set, missing
/// default, or invalid default, and editing an extract is forbidden. This
/// table is tinybrowser's own input for exactly those three facts, so the
/// generator can emit the getter instead of accepting a hand-written copy.
pub struct ReflectedEnum {
    /// IDL interface (or mixin) that declares the attribute.
    pub interface: String,
    /// IDL attribute name, e.g. `enterKeyHint`.
    pub attribute: String,
    /// The content attribute the getter reads, e.g. `enterkeyhint`.
    pub content: String,
    /// The Rust path of the `Enumerated` type, e.g. `Dir`.
    pub type_path: String,
    /// The missing-value default variant, or `None` for the spec's "no state".
    pub missing: Option<String>,
    /// The invalid-value default variant, or `None` for the spec's "no state".
    pub invalid: Option<String>,
}

/// Parse tinybrowser's reflected-enumerated table.
///
/// One entry per line: whitespace-separated `interface attribute type content
/// missing invalid`, where a default is `-` for "no state" or the variant name.
/// Blank lines and lines starting with `#` are ignored.
///
/// # Errors
/// Returns a diagnostic naming the line for a malformed entry, a duplicate
/// `(interface, attribute)` pair, or an unparsable type path or variant.
pub fn parse_reflected_enumerated(source: &Source<'_>) -> Result<Vec<ReflectedEnum>, Error> {
    crate::contracts::parse_reflected_enumerated(source)
}

/// Generated bindings for one discovered native interface implementation.
pub struct Binding {
    /// Unmodified IDL interface name, also used as the output file stem.
    pub interface: String,
    /// Rust module containing the generated trait, conversions, and dispatch.
    pub rust: String,
}

/// Compile unchanged IDL and native algorithm implementations into bindings.
///
/// Discovers `impl foo_generated::Foo<'js> for Payload` declarations in `rust`.
/// Method names follow IDL deterministically: `constructor`, `get_attribute`,
/// `set_attribute`, and snake-case operation names. Only implemented members
/// are exposed; interface constants come directly from IDL. The generated
/// trait checks algorithm signatures when the renderer compiles the output.
///
/// # Errors
/// Returns a diagnostic for invalid input, ambiguous implementations, unknown
/// implementation methods, or implemented IDL semantics not yet supported.
pub fn compile_contracts(idl: &[Source<'_>], rust: &[Source<'_>]) -> Result<Vec<Binding>, Error> {
    contracts::compile(idl, rust, &[])
}

/// Compile with tinybrowser's reflected-enumerated metadata table.
///
/// # Errors
/// Same as [`compile_contracts`].
pub fn compile_contracts_with_reflection(
    idl: &[Source<'_>],
    rust: &[Source<'_>],
    reflected: &[ReflectedEnum],
) -> Result<Vec<Binding>, Error> {
    contracts::compile(idl, rust, reflected)
}

/// JavaScript algorithms with generated interface installation contracts.
pub struct Javascript {
    /// Source with IDL-derived conversions and member mechanics inserted.
    pub source: String,
    /// Names of the interface classes discovered in this source.
    pub interfaces: Vec<String>,
}

/// Compile `__tbInstallInterface(class InterfaceName { ... })` expressions.
///
/// Class methods, getters, setters, and constructors are algorithm entry points.
/// The compiler derives their contract from the unchanged IDL corpus, validates
/// names and signatures, and supplies binding mechanics to the private installer.
/// Other JavaScript remains unchanged.
///
/// # Errors
/// Returns a diagnostic for invalid source, conflicting implementations,
/// mismatched algorithms, or implemented semantics without generated support.
pub fn compile_javascript(idl: &[Source<'_>], source: &Source<'_>) -> Result<Javascript, Error> {
    javascript::compile(idl, source)
}

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
