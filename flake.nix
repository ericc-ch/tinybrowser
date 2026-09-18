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
      # Nightly carries Miri and the sanitizer flags; builds and lint still use
      # the pinned stable toolchain above. Enter with `nix develop .#ub`.
      ubToolchain = pkgs.rust-bin.nightly.latest.default.override {
        extensions = [ "miri" "rust-src" ];
      };
      runtimeLibs = pkgs.lib.makeLibraryPath [
        pkgs.openssl
        pkgs.stdenv.cc.cc
      ];
      # cargo test binaries are unwrapped, so libssl and libstdc++ must be
      # on the loader path at runtime. Prepend so the NixOS session path
      # (gcc, GL) stays behind this shell.
      # mise rust=1 shims must not shadow the overlay toolchain.
      shellHook = ''
        export LD_LIBRARY_PATH="${runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
        export MISE_DISABLE_TOOLS="rust''${MISE_DISABLE_TOOLS:+,$MISE_DISABLE_TOOLS}"
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
            (pkgs.python3.withPackages (ps: [ ps.mako ]))
          ];
          buildInputs = [
            pkgs.openssl
          ];
          env = {
            OPENSSL_NO_VENDOR = "1";
          };
          inherit shellHook;
        };

        # Undefined-behavior tooling: Miri, sanitizers. Same build deps as the
        # default shell, nightly toolchain instead of the pinned stable one.
        ub = pkgs.mkShell {
          strictDeps = true;
          nativeBuildInputs = [
            ubToolchain
            pkgs.pkg-config
            pkgs.nodejs_24
            pkgs.valgrind
            (pkgs.python3.withPackages (ps: [ ps.mako ]))
          ];
          buildInputs = [
            pkgs.openssl
          ];
          env = {
            OPENSSL_NO_VENDOR = "1";
          };
          inherit shellHook;
        };
      };
    };
}
