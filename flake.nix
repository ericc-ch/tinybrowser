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
      runtimeLibs = pkgs.lib.makeLibraryPath [
        pkgs.openssl
        pkgs.stdenv.cc.cc
      ];
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        strictDeps = true;
        nativeBuildInputs = [
          rustToolchain
          pkgs.pkg-config
        ];
        buildInputs = [
          pkgs.openssl
        ];
        env = {
          OPENSSL_NO_VENDOR = "1";
        };
        # cargo test binaries are unwrapped, so libssl and libstdc++ must be
        # on the loader path at runtime. Prepend so the NixOS session path
        # (gcc, GL) stays behind this shell.
        # mise rust=1 shims must not shadow the overlay toolchain.
        shellHook = ''
          export LD_LIBRARY_PATH="${runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
          export MISE_DISABLE_TOOLS="rust''${MISE_DISABLE_TOOLS:+,$MISE_DISABLE_TOOLS}"
        '';
      };
    };
}
