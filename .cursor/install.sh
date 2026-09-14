#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap for tinybrowser.
#
# Two supported base environments, auto-detected:
#
#   1. Nix snapshot: if the single-user Nix profile is present, the pinned
#      toolchain (rustc/cargo 1.98 + rustfmt/clippy) and the OpenSSL that
#      native-tls links against come from the flake devshell (flake.nix,
#      rust-toolchain.toml). This is the maintainer's snapshot path.
#
#   2. Default Cursor image: rustup is already installed and honors
#      rust-toolchain.toml, so `cargo` transparently uses the pinned 1.98
#      toolchain. native-tls -> openssl-sys only needs the system OpenSSL
#      development headers + pkg-config, which we install with apt.
#
# Either way this script only refreshes repository-derived state, so it stays
# safe to run repeatedly.
set -euo pipefail

# Run from the repository root regardless of where install is invoked.
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

NIX_SH="$HOME/.nix-profile/etc/profile.d/nix.sh"

if [ -e "$NIX_SH" ]; then
  # ─── Nix snapshot path ──────────────────────────────────────────────────────
  # shellcheck source=/dev/null
  . "$NIX_SH"

  # Make `nix` (hence `nix develop`) available in future interactive shells.
  SRC_LINE='[ -e "$HOME/.nix-profile/etc/profile.d/nix.sh" ] && . "$HOME/.nix-profile/etc/profile.d/nix.sh"'
  for rc in "$HOME/.bashrc" "$HOME/.profile"; do
    touch "$rc"
    grep -qF 'nix-profile/etc/profile.d/nix.sh' "$rc" || printf '%s\n' "$SRC_LINE" >>"$rc"
  done

  # Personal agent skills from the public nixos-config repo. The repo is
  # stow-style (dotfiles/ mirrors $HOME), so the skill trees at
  # dotfiles/.agents and dotfiles/.pi are symlinked to their $HOME homes,
  # giving ~/.agents/skills/* and ~/.pi/agent/skills/*.
  NIXOS_CONFIG_DIR="$HOME/nixos-config"
  if [ -d "$NIXOS_CONFIG_DIR/.git" ]; then
    git -C "$NIXOS_CONFIG_DIR" pull --ff-only --quiet || true
  else
    git clone --depth 1 https://github.com/ericc-ch/nixos-config "$NIXOS_CONFIG_DIR"
  fi
  link_dotfile_root() {
    src="$NIXOS_CONFIG_DIR/dotfiles/$1"
    dst="$HOME/$1"
    [ -d "$src" ] || return 0
    if [ -L "$dst" ] || [ ! -e "$dst" ]; then
      ln -sfn "$src" "$dst"
    else
      echo "warn: $dst exists and is not a symlink; leaving it untouched." >&2
    fi
  }
  link_dotfile_root .agents
  link_dotfile_root .pi

  # Warm the pinned devshell and compile the workspace plus its test/example
  # targets so later `cargo test` runs are fast. Everything runs inside the
  # flake so rustc, pkg-config, OPENSSL_NO_VENDOR and LD_LIBRARY_PATH match
  # every developer invocation.
  nix develop --command cargo build --workspace --all-targets
  exit 0
fi

# ─── Default Cursor image path ────────────────────────────────────────────────
# rustup honors rust-toolchain.toml, so the first cargo invocation resolves the
# pinned 1.98 toolchain automatically. Only the OpenSSL build dependency of
# native-tls (openssl-sys) is missing from the base image.
if ! pkg-config --exists openssl 2>/dev/null; then
  echo "Installing OpenSSL development headers (openssl-sys build dependency)…"
  sudo apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq libssl-dev pkg-config
fi

# WPT (`tools/wpt/run`) needs the vendored suite. It is a large submodule, so it
# is opt-in to keep environment builds lean:
#   TINYBROWSER_INIT_WPT=1 .cursor/install.sh
if [ "${TINYBROWSER_INIT_WPT:-0}" = "1" ]; then
  echo "Initializing the WPT submodule (third_party/wpt)…"
  git submodule update --init --recursive
fi

# Compile the workspace plus its test/example targets so later `cargo test`
# runs are fast. openssl-sys locates the system OpenSSL through pkg-config, so
# no OPENSSL_NO_VENDOR / LD_LIBRARY_PATH overrides are required here.
cargo build --workspace --all-targets
