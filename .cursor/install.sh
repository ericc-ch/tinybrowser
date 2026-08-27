#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap for tinybrowser.
#
# The pinned toolchain (rustc/cargo 1.98 + rustfmt/clippy) and the OpenSSL that
# native-tls links against come from the nix flake devshell (flake.nix,
# rust-toolchain.toml). Nix itself is baked into this environment's base
# snapshot; this script only refreshes repository-derived state, so it stays
# safe to run repeatedly.
set -euo pipefail

NIX_SH="$HOME/.nix-profile/etc/profile.d/nix.sh"
if [ ! -e "$NIX_SH" ]; then
  echo "error: Nix is missing from the base image ($NIX_SH not found)." >&2
  echo "       The environment snapshot must have Nix (single-user) installed." >&2
  exit 1
fi
# shellcheck source=/dev/null
. "$NIX_SH"

# Make `nix` (hence `nix develop`) available in future interactive shells.
SRC_LINE='[ -e "$HOME/.nix-profile/etc/profile.d/nix.sh" ] && . "$HOME/.nix-profile/etc/profile.d/nix.sh"'
for rc in "$HOME/.bashrc" "$HOME/.profile"; do
  touch "$rc"
  grep -qF 'nix-profile/etc/profile.d/nix.sh' "$rc" || printf '%s\n' "$SRC_LINE" >>"$rc"
done

# Vendored html5lib tree-construction suite; browser tests assert it exists.
git submodule update --init --recursive

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
# targets so later `cargo test` runs are fast. Everything runs inside the flake
# so rustc, pkg-config, OPENSSL_NO_VENDOR and LD_LIBRARY_PATH match every
# developer invocation.
nix develop --command cargo build --workspace --all-targets
