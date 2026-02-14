# Soroban Vault STRIDE

This document captures a Soroban-specific STRIDE threat model for `contract/vault/soroban`.

## Scope

- Soroban contract entrypoints in `src/contract/mod.rs`.
- Soroban auth adapter and RBAC wiring in `src/auth/mod.rs`.
- Soroban storage serialization/versioning in `src/storage/mod.rs`.
- Market adapter interactions used by allocation/refresh/withdraw flows.
- Shared policy/auth/governance logic from `curator-primitives` crate.
- Chain-agnostic state machine and effects from `templar-vault-kernel`.

## Assets to Protect

- Underlying token balances and share accounting integrity.
- Correctness of `VaultState` and queue/order semantics.
- Authorization boundaries for curator/guardian/allocator/user actions.
- Liveness of withdrawal and refresh workflows.

## Trust Boundaries

- Off-chain operators (curator/allocator/guardian) vs on-chain runtime enforcement.
- Soroban vault contract vs external token and market adapter contracts.
- Address mapping boundary (`SdkAddress` <-> kernel `Address`).
- Stored state blob/version metadata vs runtime decode/validation.

## High-Level Dataflow

1. User signs and calls `deposit_with_min` / `request_withdraw` / `execute_withdraw`.
2. Runtime verifies auth and role policy, applies kernel action, executes effects.
3. Allocator/curator drives privileged flows via single-call methods: `allocate` (per-market supply/withdraw via kernel state machine), `refresh_markets` (query adapters and update external_assets), `set_supply_queue`, `set_paused`.
4. Storage persists `VersionedState` blob and related policy/restriction data.
5. Read APIs expose vault state and operational telemetry (including new view methods for NEAR parity: `get_fee_anchor`, `get_fees`, `get_cap_groups`, `queue_tail`, `peek_next_pending_withdrawal_id`, `get_withdrawing_op_id`, `get_current_withdraw_request_id`).

## Threats and Mitigations

| STRIDE | Soroban-specific threat | Current mitigation in code | Remaining risk / hardening |
|---|---|---|---|
| Spoofing | Caller pretends to be privileged actor for sensitive actions. | `require_auth()` and role-based authorization path (`ActionKind` -> required role) in auth/runtime. | Key compromise risk remains operational; use multisig/segregated keys for curator and delegates. |
| Tampering | Malicious adapter reports bad balances during refresh, causing bad `external_assets` sync. | `allocate` and `refresh_markets` query adapters internally and the kernel validates state transitions. Adapter responses are validated against expected totals during refresh. | Adapter correctness is still an external trust assumption; restrict adapters to vetted contracts and monitor drift. |
| Repudiation | Operators deny who executed sensitive allocator/governance actions. | Actions require signed caller auth; kernel event envelopes are emitted for state transitions. | Keep off-chain indexing/audit trails for op_id, caller, and route/context for incident forensics. |
| Information Disclosure | Queue state and config visibility leak user/operational data patterns. | No confidentiality assumptions in contract storage/events; this is expected on-chain transparency. | Treat privacy as out of scope; avoid introducing unnecessary detailed event payloads. |
| Denial of Service | Withdrawal progression stalls if allocator workflows are not executed; state blob growth pressure can make writes fail near network limits. | Queue bounded by `MAX_PENDING = 1024`; guarded state transitions; atomic tx failure prevents partial corruption. | Operate redundant keepers and alert on queue staleness; keep plans small and watch state-size/resource headroom. |
| Elevation of Privilege | Role mapping/config errors grant extra powers; reentrancy on mutating entrypoints could bypass assumptions. | Centralized action authorization; contract-level reentrancy guard wraps mutating public entrypoints. | Preserve strict role review on new entrypoints and keep reentrancy coverage for any new mutating methods. |

## Soroban-Specific Notes

- Storage decode path validates blob deserialization, version key presence, version match, and compatibility before using persisted state.
- Storage TTL must be maintained (`extend_ttl`) so persistent entries do not expire unexpectedly.
- Resource limits are network-level constraints; writes fail atomically when entry/tx limits are exceeded.
- The `#[contractimpl]` block provides the Soroban on-chain API; the `CuratorVault` runtime is chain-agnostic and reuses `curator-primitives` for auth/rbac/policy and `templar-vault-kernel` for state machine/transitions/effects/fee math.
- Kernel state machine helpers (`begin_allocating`, `finish_allocating`, `begin_refreshing`, `finish_refreshing`) are test-only (`#[cfg(any(test, feature = "testutils"))]`), not public API. Production flows use single-call methods (`allocate`, `refresh_markets`) that internally drive the kernel state machine.
- Removed methods no longer exist: `sync_external_assets`, `verify_external_assets_against_adapter`, `manual_reconcile`, `abort_allocating`, `abort_refreshing`, `abort_withdrawing`, `recover`, `settle_payout`, `refresh_fees`, and market lock methods (`acquire_market_lock`, `release_market_lock`, `is_market_locked`).

## Review Cadence

- Revisit this model when adding new privileged actions, new adapters, or storage schema changes.
- Revisit after significant Soroban SDK or network limits changes.
