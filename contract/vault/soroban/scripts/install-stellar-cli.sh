#!/usr/bin/env bash
set -euo pipefail

# ---------------------------------------------------------------------------
# Install stellar-cli v25 for Templar Soroban vault deployment.
#
# The project toolchain is pinned to Rust 1.86 (NEAR compatibility), but
# stellar-cli v25 requires Rust 1.89 to compile.  This script installs
# 1.89 as a side-by-side toolchain and builds the CLI with it.  The
# resulting binary works independently of the project toolchain.
#
# Usage:
#   ./scripts/install-stellar-cli.sh
#
# Prerequisites:
#   - rustup (https://rustup.rs)
#   - pkg-config + libdbus development headers
#       Arch/CachyOS:  pacman -S dbus pkg-config
#       Ubuntu/Debian:  apt install libdbus-1-dev pkg-config
#       Fedora:         dnf install dbus-devel pkgconf-pkg-config
#       macOS:          (not needed — dbus is optional)
# ---------------------------------------------------------------------------

STELLAR_CLI_VERSION="25.0.0"
RUST_TOOLCHAIN="1.89.0"

info()  { printf '\033[1;34m→\033[0m %s\n' "$*"; }
ok()    { printf '\033[1;32m✓\033[0m %s\n' "$*"; }
err()   { printf '\033[1;31m✗\033[0m %s\n' "$*" >&2; }

# --- preflight checks -----------------------------------------------------

if ! command -v rustup >/dev/null 2>&1; then
    err "rustup not found.  Install from https://rustup.rs"
    exit 1
fi

if ! command -v pkg-config >/dev/null 2>&1; then
    err "pkg-config not found.  See header comment for install instructions."
    exit 1
fi

# dbus is only required on Linux
if [[ "$(uname -s)" == "Linux" ]]; then
    if ! pkg-config --exists dbus-1 2>/dev/null; then
        err "libdbus development headers not found."
        err "  Arch/CachyOS:  pacman -S dbus"
        err "  Ubuntu/Debian:  apt install libdbus-1-dev"
        err "  Fedora:         dnf install dbus-devel"
        exit 1
    fi
fi

# --- install ---------------------------------------------------------------

info "Installing Rust ${RUST_TOOLCHAIN} toolchain (for CLI build only)..."
rustup toolchain install "${RUST_TOOLCHAIN}" --profile minimal 2>/dev/null || true
ok "Rust ${RUST_TOOLCHAIN} ready"

info "Building stellar-cli v${STELLAR_CLI_VERSION} (this takes ~3-4 min)..."
RUSTUP_TOOLCHAIN="${RUST_TOOLCHAIN}" cargo install --locked "stellar-cli@${STELLAR_CLI_VERSION}"
ok "stellar-cli v${STELLAR_CLI_VERSION} installed"

# --- verify ----------------------------------------------------------------

if stellar version >/dev/null 2>&1; then
    ok "$(stellar version | head -1)"
else
    # Nix/devenv users may need dbus in LD_LIBRARY_PATH
    err "stellar installed but fails to run."
    err "If using Nix/devenv, ensure 'dbus' is in LD_LIBRARY_PATH (see devenv.nix)."
    exit 1
fi
