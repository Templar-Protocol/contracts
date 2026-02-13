# Soroban Vault Runtime

This crate hosts the Soroban executor/runtime for the Templar vault kernel.

## Prerequisites

### Stellar CLI

The Stellar testnet runs protocol 25, so you need `stellar-cli` v25.  The catch:
v25 requires **Rust 1.89** to compile, but the project toolchain is pinned to
**Rust 1.86** for NEAR contract compatibility.  The solution is to build the CLI
binary with a separate 1.89 toolchain — the resulting binary works regardless of
the project toolchain.

**With devenv** (handles it automatically):

```
devenv shell
```

On first entry, devenv installs Rust 1.89 as a side-by-side toolchain and
builds `stellar-cli` v25.  Subsequent entries skip this (~3-4 min first time).

**Without devenv:**

```
./scripts/install-stellar-cli.sh
```

The script installs Rust 1.89 (via rustup) and builds the CLI.  System
prerequisites:

| OS | Packages |
|----|----------|
| Arch/CachyOS | `pacman -S dbus pkg-config` |
| Ubuntu/Debian | `apt install libdbus-1-dev pkg-config` |
| Fedora | `dnf install dbus-devel pkgconf-pkg-config` |
| macOS | (none — dbus is not needed) |

### Nix / devenv note

The nix environment isolates libraries from the host.  If `stellar` segfaults or
reports `libdbus-1.so.3: cannot open`, ensure `dbus` is in the devenv
`LD_LIBRARY_PATH` (already configured in `devenv.nix`).

## Quick start (testnet)

```
cd contract/vault/soroban

just setup              # build WASM, configure network, create & fund identity
just deploy-all         # deploy vault + test token, initialize
just demo-deposit       # deposit test tokens
just demo-withdraw      # request and execute withdrawal
```

All recipes are in the `justfile`.  Run `just --list` for the full catalogue.

The build step compiles the WASM and runs `stellar contract optimize` to shrink
it from ~430KB to ~250KB (required to stay under Soroban's transaction limits).

## State Size and Operational Limits

- Soroban enforces per-entry and per-transaction resource limits. Current network values are documented by Stellar: https://developers.stellar.org/docs/networks/resource-limits-fees
- Vault runtime state is persisted as a single `StateBlob`, so serialized `VaultState` size is the practical storage-pressure point.
- The main long-lived growth vector is pending withdrawals, which are bounded by `MAX_PENDING = 1024`.
- In-flight operation plans (`Allocating.plan`, `Refreshing.plan`) are expected to remain small under allocator policy, so the 1024 pending-withdrawal cap is the dominant operational bound in practice.

## Practical Risk Model

- TVL growth by itself does not significantly increase serialized state size.
- Risk comes from queue backlog plus unusually large in-flight plans.
- If state exceeds Soroban storage write limits, the transaction fails atomically (no partial state commit).

## Parity Tests

Parity tests check behavioral equivalence across the shared kernel and chain executors (NEAR and Soroban). They ensure state transitions, accounting behavior, and invariants stay aligned as implementations evolve.

- Guide: `contract/vault/README.md#parity-tests`

## Threat Model

- Soroban-specific STRIDE: `contract/vault/soroban/STRIDE.md`
