# Halborn Audit Merge Decisions

Local rehearsal branch: `halborn-audit`

Base used: `origin/spr/refactor/vault-ergonomics/4f330057` at `4e72696d27e0f716bf0ffa9b7776bc593f581c71`.

## 2026-06-02

### PR #427 over PR #425/#426/#431

- Preserved PR #426 keyed `AdapterBindings` and typed supply-queue adapter accounts.
- Did not restore PR #427's older positional `AllowedAdapters` length check against the supply queue. A-001 explicitly changed allowed adapters into an allowlist, not a positional mirror.
- Combined governance action handling so `SetSupplyQueue(_, _)` remains typed while PR #427's `SetAllocators`, `SetAllowedAdapters`, `Upgrade`, `Migrate`, and `CancelMigration` actions stay timelocked/routed.
- Kept PR #427 caller preauthorization and sentinel/governance identity helpers.
- Combined storage-test imports so both adapter-binding regressions and governance control-plane regressions compile.

### PR #451 over integrated #427

- Kept PR #451 governance target validation for `SetGovernance`.
- Preserved PR #426 adapter-aware `SetSupplyQueue(target_ids, adapters)` validation, including duplicate target detection and adapter contract-address validation.
- Removed dead helper code from PR #451 that supported its older positional `adapter_for_market` path; the integrated branch keeps keyed adapter bindings.
- Kept #451 role-topology address restrictions via `require_wasm_or_account_address`.
- Kept both test helper families: manual runtime storage setup for adapter-binding tests and registered governance-contract setup for role-topology tests.

### PR #448 over integrated #451

- Changed `apply_supply_queue_policy` to accept both explicit adapter accounts and `caller_preauthorized`.
- Preserved PR #448's preauthorized governance execution path while keeping PR #426 adapter binding semantics.

### PR #455 over integrated #448

- Kept PR #455 strict wire-shape validation with `ensure_some`/`ensure_none`.
- Allowed `accounts` for `GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE` because PR #426 uses that field to carry typed adapter accounts.
- Continued rejecting unrelated optional fields on supply-queue commands.

### PR #436 over integrated governance stack

- Preserved all adjacent tests: PR #448 proposal-kind ambiguity/timelock policy tests and PR #436 timelock getter non-materialization regression.

### PR #437 in progress

- Kept PR #437 pending-queue cap and paged pending-storage model.
- Kept PR #437 scoped revocation authorization, while preserving PR #448 duplicate-pending ambiguity guard in `revoke_kind`.
- Dropped the obsolete guardian config path during conflict resolution because the integrated shared-types ABI no longer defines `GOVERNANCE_CONFIG_KIND_GUARDIANS` and the governance action/types no longer include guardian proposals.
- Removed `migrate_legacy_paused` from imports because PR #437 removes the legacy paused migration shim.
- Updated downstream runtime tests to match the removed guardian config ABI: `ContractConfig::new` no longer receives a guardian list, and the SAC-role rejection parameterized test now covers primary roles via curator/sentinel and list roles via allocators.

### PR #441 in progress

- Kept both runtime ABI regressions: unauthorized governance callers are rejected before malformed body decode, and group-membership governance commands require an explicit `market_id`.
- Used the integrated registered-governance test helper for the #441 group-membership test instead of the older generated-address-only setup, so the test matches the post-#451 governance identity requirements.
- Preserved PR #441's explicit `market_id` unwrapping for membership mode and removed a stale second `ok_or` on the already-unwrapped `u32`.

### PR #428 in progress

- Kept both kernel invalid-state variants added by earlier PRs: `WithdrawalLiquidityBelowMinimum = 41` from PR #425 and `RequestWithdrawExpectedAssetsExceedTotalAssets = 42` from PR #428, avoiding duplicate diagnostic codes.
- Added PR #428 fee-anchor helpers (`current_idle_assets`, idle reconciliation, virtual-offset lock checks, fee-anchor normalization) while preserving the integrated governance/role helper imports.
- Did not reintroduce `migrate_legacy_paused`; PR #437 removed that legacy paused-state migration shim. Kept `normalize_fee_anchor()` during `migrate()`.
- Preserved PR #425's structured `ExecuteWithdrawStatus` return and added PR #428's separate `RefreshFees` implementation, instead of taking PR #428's older unit-returning `execute_withdraw_impl`.
- In the Soroban integration fixture, kept the registered governance contract from the governance stack and PR #428's `asset_token` fixture field.

### PR #430 in progress

- Kept PR #430's `validate_and_rewrite_storage()` call during governed `migrate()` so current-version state, policy, restrictions, and fee blobs are validated/re-written.
- Ordered governed `migrate()` as fee-anchor normalization first, then storage validation/rewrite, then TTL extension and migration flag clearing.
- Did not reintroduce PR #430's older `migrate_legacy_paused` helper or `LEGACY_PAUSED_MIGRATED_KEY`, because PR #437 intentionally removed the undeployed legacy paused migration path.
- Kept bounded conversion helpers and the storage queue cap import together in `contract/mod.rs`.
- Combined storage-test imports so adapter/governance tests and PR #430 paged-storage cap tests both compile.
- Removed the stale `MAX_PENDING` import after PR #430 switched the Soroban runtime path to `SOROBAN_MAX_PENDING_WITHDRAWALS`.
- Renamed the now-unused `proxy_view` owner parameter to `_owner`; the ABI-shaped argument remains present while the merged fee/proxy-view logic no longer reads it directly.

### PR #429 in progress

- For share-token vault address validation, took PR #429's stricter `AddressPayload::ContractIdHash` check instead of the current branch's broader `Executable::{Wasm, StellarAsset}` check.
- Kept `symbol_short` because PR #429 emits the delegated `burn_from` observability event.

### PR #452 in progress

- For Blend adapter, kept PR #431's removal of public admin/accounting shortcuts (`supply_balance`, `withdraw_to_vault`) and did not add PR #452's older admin-rotation entrypoints.
- Kept PR #452 Blend adapter pause/readback and upgrade observability controls because they do not reopen the adapter accounting shortcut boundary.
- Combined Blend adapter tests by keeping PR #431 source-sweep tests and PR #452 upgrade event-count coverage, while removing stale admin-rotation tests for entrypoints not present in the integrated branch.
- For share-token admin rotation, combined PR #429 vault-admin validation with PR #452's pending-admin flow: proposed admins are validated against the current vault before being stored as pending.
