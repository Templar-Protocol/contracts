# 2. Expose owner-controlled RedStone adapter administration

Date: 2026-11-05

## Status

Proposed

## Context

The RedStone adapter needs an administration interface for changing its verification policy and
upgrading its code. The owner may be an account or a contract; choosing a DAO or a particular
governance implementation is a separate integration decision.

PR #709 proposed this interface before the ADR process landed. This record makes that pending
decision explicit; it does not accept it or authorize a live deployment. The proposal snapshot is
commit `d743e76e5c848524f12a4e933dbe9d74dc0fbe8e`:

- `contract/redstone-adapter/src/lib.rs:26-92` separates ownership from RBAC and exposes the
  configuration, upgrade, and migration methods.
- `common/src/oracle/redstone/config/mod.rs:13-38` retains the configuration representation and
  validates the signer policy without validating historical state during decoding.
- `common/src/oracle/redstone/event.rs:12-47` preserves `write_prices` and adds configuration
  observability; `common/src/upgrade.rs:58-78` supplies the existing atomic self-upgrade primitive.

| Scope | Disposition |
|---|---|
| Owner administration, full configuration replacement, upgrade/bootstrap, audit event | Include as one adapter-interface decision |
| Focused contract and released-WASM migration proofs | Include; verify the adapter directly |
| DAO selection, proxy-oracle governance integration, timelocks, deployment and key removal | Defer; no controller topology is selected |
| Partial configuration patches and a new patch-input format | Reject; use the existing complete `Config` |
| Dependency or Rust-toolchain upgrades | Exclude from this change |

## Decision

1. Use the existing standard `Owner` component. New construction validates configuration and gives
   the explicit `admin_id` ownership while preserving its existing role grants. Ownership controls
   configuration and upgrades; `ModifyRoles` and `TrustedUpdater` remain independent. Ownership
   changes do not move roles or remove account keys.
2. Expose `admin_set_config(config: Config)`, requiring the owner and exactly one yoctoNEAR.
   Validate before replacing the complete configuration. Require 1-255 distinct, nonzero signer
   addresses and a positive quorum no greater than the signer count. Preserve the existing full
   `u64` timing domains. There is no merge, revision counter, or compare-and-swap: a later call
   replaces every field, including fields copied from an older snapshot.
3. Keep changes prospective. Do not clear cached prices, replay timestamps, or downstream caches,
   and do not change the fixed stored-price staleness limit or existing price-processing rules.
4. Expose owner-only, one-yocto `admin_upgrade(code, migrate_args)` using the existing
   `UpgradeSource`: inline WASM or an immutable global code hash, never a mutable account reference.
   Deploy and migrate in one self-receipt; failed migration reverts that receipt's code/state.
   Reserve 250 Tgas for migration. Network gas feasibility remains a deployment check.
5. Expose nonpayable, self-only `migrate(owner_id: Option<AccountId>)`. When the Owner initialization
   sentinel is absent, require an explicit owner and initialize it once. When present, reject a
   supplied owner; no owner argument is a no-op, preserving pending transfers and renunciation.
   Preserve the adapter-only root, feed and RBAC layouts, historical configuration, and records.
6. Emit `config_updated` version `1.0.0` under `redstone-adapter` after a successful replacement,
   carrying the actual predecessor and complete applied configuration. Encode event signers as
   ordered 40-character lowercase hex strings to fit the log budget. Keep configuration input,
   views, storage and the existing `write_prices` event unchanged. Use standard Owner events for
   ownership changes. Consumers must check the emitting receipt succeeded.
7. Keep the interface controller-neutral. Do not extend the proxy-oracle governance contract,
   prescribe its use, or add proposal/policy scenarios for that hypothetical integration.

## Alternatives

- Partial patches preserve unmentioned live settings, but introduce another input type, merge
  semantics, and coupled validation paths. Full replacement keeps the interface smaller at the
  explicit cost of reviewing complete snapshots and execution order.
- Reusing `ModifyRoles` for all administration avoids separate ownership, but couples writer-role
  administration to configuration and arbitrary code deployment. Separate ownership follows the
  existing adapter convention and keeps those capabilities distinct.
- Selecting a DAO or the proxy-oracle governance contract now would prove one deployment topology,
  but there is no current requirement to choose it. Test the adapter boundary directly and review
  a controller integration when one is actually selected.

## Consequences

The owner is trusted to select valid policies and deploy arbitrary code. This interface supplies
neither a timelock nor a code allowlist, and transferring ownership alone is not a complete
authority handover. Renunciation on a keyless adapter disables owner administration without a
re-bootstrap escape hatch.

Acceptance requires:

- Fast tests for validation, owner/deposit boundaries, atomic rejection, full replacement,
  unchanged cached/replay state, migration sentinel behavior, and exact event/storage wire forms.
- Focused direct-WASM tests for real released `0.1.0` bootstrap preservation and failure rollback,
  the generated configuration boundary, and owner-driven inline/global upgrades. Do not replace
  the removed governance fixture with another controller framework.
- Maximum-policy logging remains complete within the receipt log limit, with no configuration
  event advertised by a rejected call.
- No production governance/gateway changes, dependency-version changes, or toolchain update.

The useful local checks are `just test-fast -p templar-common --lib`,
`just test-fast -p templar-redstone-adapter-contract`, the focused direct sandbox target, a WASM
build/check, and the repository's affected-target lint and documentation checks. Testnet and
production execution are not authorized by this decision; verify target-network limits, selected
authority, external event consumers and any monitoring rules before a separately approved rollout.
An emitted event provides observability, not an installed alert.

| Failure scenario | Warning and prevention | Detection and recovery |
|---|---|---|
| An old full snapshot restores stale settings | Review every field and execution order, not only the intended difference | Compare `get_config` and successful `config_updated` records; submit a fresh complete configuration through the current owner |
| A wrong bootstrap owner or renunciation loses administration | Verify owner and pending owner before any separately approved key removal; never use renunciation for handover | Inspect ownership and remaining authorities; use retained deployment authority for reviewed recovery before key removal, otherwise recovery may be impossible |
| Deployment succeeds but migration fails | Keep both actions in one self-receipt and verify code source and target-network gas feasibility | Inspect receipt failures and code/config/role/feed snapshots; the failed receipt preserves prior code/state, so retry only after correcting the source, arguments or budget |

Future root-layout changes require their own migration decision; this bootstrap is not a general
state-versioning mechanism. A future controller integration and private deployment choices belong
in their own review, not in this adapter-interface change.
