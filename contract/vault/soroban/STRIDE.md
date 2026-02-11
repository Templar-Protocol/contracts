# Soroban Vault STRIDE

This document captures a Soroban-specific STRIDE threat model for `contract/vault/soroban`.

## Scope

- Soroban contract entrypoints in `src/contract.rs`.
- Soroban auth adapter and RBAC wiring in `src/auth.rs`.
- Soroban storage serialization/versioning in `src/storage.rs`.
- Market adapter interactions used by allocation/refresh/withdraw flows.

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

1. User signs and calls `deposit_with_min` / `request_withdraw`.
2. Runtime verifies auth and role policy, applies kernel action, executes effects.
3. Allocator/curator drives privileged flows (`begin_allocating`, `begin_refreshing`, `sync_external_assets`, finalize/abort paths).
4. Storage persists `VersionedState` blob and related policy/restriction data.
5. Read APIs expose vault state and operational telemetry.

## Threats and Mitigations

| STRIDE | Soroban-specific threat | Current mitigation in code | Remaining risk / hardening |
|---|---|---|---|
| Spoofing | Caller pretends to be privileged actor for sensitive actions. | `require_auth()` and role-based authorization path (`ActionKind` -> required role) in auth/runtime. | Key compromise risk remains operational; use multisig/segregated keys for curator and delegates. |
| Tampering | Malicious adapter reports bad balances during refresh, causing bad `external_assets` sync. | `sync_external_assets` verification rejects adapter unavailable/partial failure and mismatched claimed totals during refresh. | Adapter correctness is still an external trust assumption; restrict adapters to vetted contracts and monitor drift. |
| Repudiation | Operators deny who executed sensitive allocator/governance actions. | Actions require signed caller auth; kernel event envelopes are emitted for state transitions. | Keep off-chain indexing/audit trails for op_id, caller, and route/context for incident forensics. |
| Information Disclosure | Queue state and config visibility leak user/operational data patterns. | No confidentiality assumptions in contract storage/events; this is expected on-chain transparency. | Treat privacy as out of scope; avoid introducing unnecessary detailed event payloads. |
| Denial of Service | Withdrawal progression stalls if allocator workflows are not executed; state blob growth pressure can make writes fail near network limits. | Queue bounded by `MAX_PENDING = 1024`; guarded state transitions; atomic tx failure prevents partial corruption. | Operate redundant keepers and alert on queue staleness; keep plans small and watch state-size/resource headroom. |
| Elevation of Privilege | Role mapping/config errors grant extra powers; reentrancy on mutating entrypoints could bypass assumptions. | Centralized action authorization; contract-level reentrancy guard wraps mutating public entrypoints. | Preserve strict role review on new entrypoints and keep reentrancy coverage for any new mutating methods. |

## Soroban-Specific Notes

- Storage decode path validates blob deserialization, version key presence, version match, and compatibility before using persisted state.
- Storage TTL must be maintained (`extend_ttl`) so persistent entries do not expire unexpectedly.
- Resource limits are network-level constraints; writes fail atomically when entry/tx limits are exceeded.

## Review Cadence

- Revisit this model when adding new privileged actions, new adapters, or storage schema changes.
- Revisit after significant Soroban SDK or network limits changes.
