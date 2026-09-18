//! Link-time size knobs for the shipping binary.
//!
//! These lived in `.cargo/config.toml` as global `rustflags`. They are scoped
//! to the release binary instead: they only shrink the shipping artifact, and
//! applying them to every debug/test link forces LLD onto objects whose DWARF
//! rustc 1.98.1 does not always compile to something LLD accepts (`unknown
//! relocation (1875)` on `icu_experimental`); the default bfd linker ignores
//! it. Nothing unwinds in release (`panic = "abort"`, QuickJS-ng uses
//! setjmp/longjmp), so the shipping binary needs neither the unwind index
//! (dropped here) nor the `.eh_frame` body (`tools/ship` removes it).
//!
//! Sizes and method: `docs/researches/size-budget.md`.

fn main() {
    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }
    for arg in [
        "-fuse-ld=lld",
        "-Wl,--icf=all",
        "-Wl,--build-id=none",
        "-Wl,--gc-sections",
        "-Wl,-O2",
        "-Wl,-z,pack-relative-relocs",
        "-Wl,--no-eh-frame-hdr",
    ] {
        println!("cargo:rustc-link-arg-bins={arg}");
    }
}
