#!/usr/bin/env bash
set -euo pipefail

# ---------------------------------------------------------------------------
# Install stellar-cli v26 for Templar Soroban contract deployment.
#
# CI uses the official release binary. Building stellar-cli from source took
# over 12 minutes on PR #472's GitHub runner, so source installation is now an
# explicit local fallback via STELLAR_CLI_INSTALL_MODE=source.
#
# Usage:
#   ./script/soroban/install-stellar-cli.sh
#
# Prerequisites:
#   - curl + tar for release binary installation
#   - source mode additionally requires rustup + pkg-config + libdbus/libudev
#     development headers:
#       Arch/CachyOS:  pacman -S dbus systemd pkg-config
#       Ubuntu/Debian: apt install libdbus-1-dev libudev-dev pkg-config
#       Fedora:        dnf install dbus-devel systemd-devel pkgconf-pkg-config
# ---------------------------------------------------------------------------

STELLAR_CLI_VERSION="26.0.0"
RUST_TOOLCHAIN="1.92.0"
INSTALL_MODE="${STELLAR_CLI_INSTALL_MODE:-binary}"

info()  { printf '\033[1;34m→\033[0m %s\n' "$*"; }
ok()    { printf '\033[1;32m✓\033[0m %s\n' "$*"; }
err()   { printf '\033[1;31m✗\033[0m %s\n' "$*" >&2; }

# --- helpers ---------------------------------------------------------------

install_dir() {
    if [[ -n "${CARGO_HOME:-}" ]]; then
        printf '%s/bin\n' "$CARGO_HOME"
    else
        printf '%s/.cargo/bin\n' "$HOME"
    fi
}

target_triple() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"

    case "${os}:${arch}" in
        Linux:x86_64) printf 'x86_64-unknown-linux-gnu\n' ;;
        Linux:aarch64 | Linux:arm64) printf 'aarch64-unknown-linux-gnu\n' ;;
        Darwin:x86_64) printf 'x86_64-apple-darwin\n' ;;
        Darwin:aarch64 | Darwin:arm64) printf 'aarch64-apple-darwin\n' ;;
        *)
            return 1
            ;;
    esac
}

installed_stellar_matches() {
    command -v stellar >/dev/null 2>&1 \
        && stellar version 2>/dev/null | grep -Fq "stellar ${STELLAR_CLI_VERSION}"
}

install_binary() {
    if ! command -v curl >/dev/null 2>&1; then
        err "curl not found."
        exit 1
    fi
    if ! command -v tar >/dev/null 2>&1; then
        err "tar not found."
        exit 1
    fi

    local triple url tmp bin_dir
    if ! triple="$(target_triple)"; then
        err "No prebuilt stellar-cli v${STELLAR_CLI_VERSION} asset for $(uname -s)/$(uname -m)."
        err "Set STELLAR_CLI_INSTALL_MODE=source to compile locally."
        exit 1
    fi

    url="https://github.com/stellar/stellar-cli/releases/download/v${STELLAR_CLI_VERSION}/stellar-cli-${STELLAR_CLI_VERSION}-${triple}.tar.gz"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' RETURN
    bin_dir="$(install_dir)"
    mkdir -p "$bin_dir"

    info "Downloading stellar-cli v${STELLAR_CLI_VERSION} (${triple})..."
    curl --fail --location --silent --show-error "$url" | tar -xz -C "$tmp"
    install -m 0755 "$tmp/stellar" "$bin_dir/stellar"
    export PATH="${bin_dir}:${PATH}"
    ok "stellar-cli v${STELLAR_CLI_VERSION} installed to ${bin_dir}/stellar"
}

install_source() {
    if ! command -v rustup >/dev/null 2>&1; then
        err "rustup not found.  Install from https://rustup.rs"
        exit 1
    fi

    if ! command -v pkg-config >/dev/null 2>&1; then
        err "pkg-config not found.  See header comment for install instructions."
        exit 1
    fi

    # dbus and libudev are required by stellar-cli's default additional-libs feature.
    if [[ "$(uname -s)" == "Linux" ]]; then
        if ! pkg-config --exists dbus-1 2>/dev/null; then
            err "libdbus development headers not found."
            err "  Arch/CachyOS:  pacman -S dbus"
            err "  Ubuntu/Debian: apt install libdbus-1-dev"
            err "  Fedora:        dnf install dbus-devel"
            exit 1
        fi
        if ! pkg-config --exists libudev 2>/dev/null; then
            err "libudev development headers not found."
            err "  Arch/CachyOS:  pacman -S systemd"
            err "  Ubuntu/Debian: apt install libudev-dev"
            err "  Fedora:        dnf install systemd-devel"
            exit 1
        fi
    fi

    info "Installing Rust ${RUST_TOOLCHAIN} toolchain (for CLI build only)..."
    rustup toolchain install "${RUST_TOOLCHAIN}" --profile minimal 2>/dev/null || true
    ok "Rust ${RUST_TOOLCHAIN} ready"

    info "Building stellar-cli v${STELLAR_CLI_VERSION} from source..."
    RUSTUP_TOOLCHAIN="${RUST_TOOLCHAIN}" cargo install --locked "stellar-cli@${STELLAR_CLI_VERSION}"
    ok "stellar-cli v${STELLAR_CLI_VERSION} installed"
}

# --- install ---------------------------------------------------------------

if installed_stellar_matches; then
    ok "$(stellar version | head -1) already installed"
elif [[ "$INSTALL_MODE" == "source" ]]; then
    install_source
else
    install_binary
fi

# --- verify ----------------------------------------------------------------

if stellar version >/dev/null 2>&1; then
    ok "$(stellar version | head -1)"
else
    # Nix/devenv users may need dbus in LD_LIBRARY_PATH
    err "stellar installed but fails to run."
    err "If using Nix/devenv, ensure 'dbus' is in LD_LIBRARY_PATH (see devenv.nix)."
    exit 1
fi
