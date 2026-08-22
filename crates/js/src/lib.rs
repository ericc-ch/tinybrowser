//! `QuickJS` embed and native DOM bindings.
//!
//! Op bodies are wrapped so a panic degrades to an error return instead of
//! unwinding across the FFI boundary into `QuickJS`.
//!
//! Fetch capability is injected from above via a consumer-side trait; this
//! crate must not depend on `net`.
