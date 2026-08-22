//! `PageActor` and navigation lifecycle: sessions, waits, wiring.
//!
//! The single fan-in point over `dom`, `net`, and `js`. Injects the HTTP
//! adapter into the JS runtime. Everything above it goes through here; when
//! CDP arrives it will depend on this crate alone.
