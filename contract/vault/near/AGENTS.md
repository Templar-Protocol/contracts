# NEAR Vault Contract Agent Guide

This file contains NEAR-vault-specific guidance for future agents. The Soroban
vault has its own guide at `contract/vault/soroban/AGENTS.md`.

## Scope

The deployable NEAR vault contract in `contract/vault/near` is an ERC-4626-style
vault that issues NEP-141 shares over a `BorrowAsset` and allocates idle balance
across markets. Share accounting, the withdraw queue, and most invariants live in
`templar-common` and `templar-vault-kernel`; the deployable contract adds the
NEP-141/NEP-145 surface, async orchestration, and storage management.

Read these together before non-trivial changes:

- `contract/vault/near/src/lib.rs`
- `contract/vault/near/src/governance.rs`
- `contract/vault/near/src/impl_callbacks.rs`
- `contract/vault/near/src/impl_token_receiver.rs`
- `contract/vault/near/src/kernel_effects.rs`
- `contract/vault/near/src/storage_management.rs`
- `common/src/vault/mod.rs`

## What is actually callable on-chain

A method is callable on-chain **only** if it is a `pub fn` inside a
`#[near]`-annotated impl block. A `pub fn` in a plain `impl Contract` (no `#[near]`),
or in a trait impl whose `impl` is not `#[near]`-annotated, is **not** exported.

The vault's exporting blocks are:

- `lib.rs` — `#[near] impl Contract` for writes (~`lib.rs:295`) and for views
  (~`lib.rs:1037`)
- `governance.rs` — `#[near]` impl (~`governance.rs:332`)
- `impl_callbacks.rs` — `#[near]` impl (~`impl_callbacks.rs:86`)
- `impl_token_receiver.rs` — `#[near]` impls (~`impl_token_receiver.rs:40` and `:68`)

Notably, `impl VaultExternalInterface for Contract` in
`contract/vault/near/src/impl_vault_external.rs` is **not** `#[near]`-annotated, so
the `pub fn`s there (e.g. `get_fee_anchor_timestamp`) are internal delegates, not
exported entrypoints. Likewise `internal_accrue_fee` lives in a plain
`impl Contract` and is not exported.

## Keeping the gateway in sync (important — not matched by ast-grep)

The vault's public surface is mirrored in the gateway in **three** places that
must stay aligned with the `#[near]` blocks above:

- `gateway/methods-spec/src/vault.rs` — typed op structs + registration in
  `for_each_read_method!` / `for_each_write_method!`
- `gateway/methods-dispatch/src/vault_impl.rs` — `DispatchRead` / `PlanWrite` impls
  mapping each op to a contract call
- `gateway/core/src/client/vault.rs` — the bound `VaultClient`, which declares the
  contract methods inside the `contract_views!` / `contract_writes!` **macro DSL**

Why this needs a written reminder: the client lists method names *inside a macro
DSL*, so ast-grep / structural search will **not** match them. A gateway method
that dispatches to a contract method which is not actually exported compiles
cleanly and only fails at runtime with a "method not found" error. This is exactly
what ENG-392 (PR #486) had to fix:

- `vault.getFeeAnchorTimestamp` dispatched to the non-exported
  `get_fee_anchor_timestamp` — re-routed through the exported `get_fee_anchor` view.
- `vault.accrueFee` dispatched to the non-exported `internal_accrue_fee` — removed
  (accrual already runs inside the other ops).

Rules of thumb:

- Only dispatch the gateway to methods inside a `#[near]` impl block. When unsure,
  confirm the method's impl block carries `#[near]`.
- When you add, remove, or rename an exported vault method, update all three
  gateway files above and regenerate the catalog:
  `cargo test -p templar-gateway-catalog regenerate_methods_md -- --ignored`.
- Numeric values crossing the gateway are string-encoded (`SU64` / `SU128`), not
  raw `u64` / `u128`, for JS safety. Keep new vault ids/amounts consistent.

## NEAR-specific notes

- Withdraw/redeem are asynchronous: `redeem` escrows shares to the vault itself
  (the vault must be registered on its own share token) and creates a request;
  payout happens later via the underlying token's `ft_transfer`, which fails on an
  unregistered receiver and refunds the escrowed shares. The gateway withdraw/redeem
  plans pre-register both the vault (on its share token) and the receiver (on the
  underlying) for this reason — see `withdraw_registration_steps` in
  `gateway/methods-dispatch/src/vault_impl.rs`.
- `set_supply_queue` charges storage only for markets newly added to the queue
  (`storage_management::yocto_for_queue_additions`) and does not refund removals or
  reorders.

## Documentation Maintenance

- Update this file when NEAR-vault assumptions, exported-method conventions, or the
  gateway-mirror contract change.
