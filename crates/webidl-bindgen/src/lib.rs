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
    contracts::compile(idl, rust)
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
