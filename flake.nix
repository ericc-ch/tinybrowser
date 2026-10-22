{
  description = "Pinned rustc and OpenSSL for native-tls builds";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
      rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      # Pinned nightly (rust-toolchain.toml): `[profile.release] rustflags`
      # (unwind tables off) and `.cargo/config.toml`'s in-tree std both need a
      # nightly toolchain with `rust-src`.
      runtimeLibs = pkgs.lib.makeLibraryPath [
        pkgs.openssl
        pkgs.fontconfig
        pkgs.stdenv.cc.cc
      ];
      # cargo test binaries are unwrapped, so libssl and libstdc++ must be
      # on the loader path at runtime. Prepend so the NixOS session path
      # (gcc, GL) stays behind this shell.
      # mise rust=1 shims must not shadow the overlay toolchain.
      # One target dir per checkout, on the SSD. Cargo holds an exclusive lock
      # on the target dir for the whole build, so a machine-wide one serialized
      # every worktree behind the same lock; and path dependencies change their
      # package id per checkout path, so the artifacts were never reused anyway.
      shellHook = ''
        export LD_LIBRARY_PATH="${runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
        export MISE_DISABLE_TOOLS="rust''${MISE_DISABLE_TOOLS:+,$MISE_DISABLE_TOOLS}"
        export CARGO_TARGET_DIR="/mnt/ssd/rust-targets/$(basename "$(git rev-parse --show-toplevel 2>/dev/null || echo "$PWD")")"
      '';
    in
    {
      devShells.${system} = {
        default = pkgs.mkShell {
          strictDeps = true;
          nativeBuildInputs = [
            rustToolchain
            pkgs.pkg-config
            pkgs.nodejs_24
            pkgs.valgrind
            (pkgs.python3.withPackages (ps: [ ps.mako ]))
          ];
          buildInputs = [
            pkgs.openssl
            pkgs.fontconfig
          ];
          env = {
            OPENSSL_NO_VENDOR = "1";
          };
          inherit shellHook;
        };
      };
    };
}
