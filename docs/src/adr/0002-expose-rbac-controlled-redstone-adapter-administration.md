# 2. Expose RBAC-controlled RedStone adapter administration

Date: 2026-11-05

## Status

Accepted

## Context

The RedStone adapter needs an administration interface for changing its verification policy and
upgrading its code. The existing role administrator should hold those powers; a second Owner
authority and an ownership bootstrap are unnecessary. Choosing a DAO or a particular governance
implementation remains a separate integration decision.

PR #709 originally proposed separate ownership and a handwritten migration before the ADR process
landed. This revised Proposed decision replaces that approach with Admin RBAC and the repository's
versioned-state convention. Earlier reviews of the Owner-based candidate do not approve this
revision, and this record does not authorize deployment.

Evidence at implementation snapshot `4aac49a41c0ae9e355d08e8ed9f7ea58d6fdef1a`:

- `common/src/oracle/redstone/adapter.rs:17-28` defines the existing role order and adapter layout.
  `near-sdk-contract-tools` 4.0.0 keys RBAC memberships by the Borsh-serialized role.
- `contract/redstone-adapter/src/lib.rs:26-92` contains the separate Owner and custom migration to
  replace; its full configuration validation and audit event remain useful.
- `common/src/versioned_state/core.rs:6-27,73-102` treats an absent `__v` key as version 0 and
  supplies typed state transformers. `common/src/versioned_state/macros.rs:23-69` supplies the
  private migration entrypoint and validates the complete migration chain before running it.
- `contract/pyth-lazer/contract/src/lib.rs:299-307` uses `VersionedState<State>` and
  `impl_versioned_state!`; the proxy oracle's `state/legacy/v0.rs` and `migration/v0_to_v1.rs`
  demonstrate a typed legacy migration. `common/src/upgrade.rs:58-78` supplies atomic self-upgrade.
- `gateway/methods-spec/src/redstone.rs:43-55` and
  `tools/manager/src/commands/redstone/mod.rs:39-52` expose role names to Rust, JSON and CLI users.

Read-only mainnet inspection at finalized block 223219057 found that the known active deployment,
`redstone-adapter.v1.tmplr.near`, matches the catalog-pinned RedStone 0.1.0 WASM, has no `__v` or
standard Owner entries, and has only `v1.tmplr.near` in `ModifyRoles`. The historical binary used by
the migration proof therefore matches this deployment; no mainnet state was changed.

| Scope | Disposition |
|---|---|
| Admin RBAC, compatible role aliases, full configuration replacement, versioned migration, audit event | Include as one adapter-interface decision |
| Focused contract, compatibility and released-WASM migration proofs | Include; verify the adapter directly |
| DAO selection, proxy-oracle governance integration, timelocks, deployment and key removal | Defer; no controller topology is selected |
| Partial configuration patches and a new patch-input format | Reject; use the existing complete `Config` |
| Dependency or Rust-toolchain upgrades | Exclude from this change |

## Decision

1. Use existing RBAC for all administration. Call the capability `Admin`: it controls role
   management, configuration and code upgrades. Keep `TrustedUpdater` separate and preserve the
   existing role-removal flags and last-member guard. New construction validates configuration
   and grants both roles to explicit `admin_id`, not the predecessor. Remove the proposed Owner
   derive, ownership methods and ownership-bootstrap parameter; introduce no Owner storage.
2. Expose `admin_set_config(config: Config)`, requiring Admin and exactly one yoctoNEAR.
   Validate before replacing the complete configuration. Require 1-255 distinct, nonzero signer
   addresses and a positive quorum no greater than the signer count. Preserve the existing full
   `u64` timing domains. There is no merge, revision counter, or compare-and-swap: a later call
   replaces every field, including fields copied from an older snapshot.
3. Keep changes prospective. Do not clear cached prices, replay timestamps, or downstream caches,
   and do not change the fixed stored-price staleness limit or existing price-processing rules.
4. Expose Admin-only, one-yocto `admin_upgrade(code, migrate_args)` using the existing
   `UpgradeSource`: inline WASM or an immutable global code hash, never a mutable account reference.
   Deploy and migrate in one self-receipt; failed migration reverts that receipt's code/state.
   Reserve 250 Tgas for migration. Network gas feasibility remains a deployment check.
5. Adopt `VersionedState<State>` with target version 1 and `impl_versioned_state!`, including its
   standard version views. A typed V0-to-V1 transformer reads the legacy adapter-only root and
   moves the existing adapter into the current state. Preserve historical configuration without
   revalidation, cached feeds, replay timestamps, feed prefix `a`, and RBAC prefix `~r`; do not
   recreate or iterate feed collections or reseed roles. Only the state-version metadata is new.
   Fresh construction records version 1. The generated self-only migration accepts the standard
   typed migration arguments when needed and empty raw bytes when already current, not `{}`.
   There is no `owner_id` argument or bespoke migration dispatch.
6. Emit `config_updated` version `1.0.0` under `redstone-adapter` after a successful replacement,
   carrying the actual predecessor and complete applied configuration. Encode event signers as
   ordered 40-character lowercase hex strings to fit the log budget. Keep configuration input,
   views, storage and the existing `write_prices` event unchanged. Consumers must check the
   emitting receipt succeeded. No ownership events or handover interface are introduced.
7. Preserve compatibility while introducing the Admin name. Keep the public Rust enum variant
   `ModifyRoles` and expose `Role::Admin` as a constant alias for the same value; do likewise for
   the gateway role value. This preserves legacy enum imports and exhaustive matches, not just
   serialization. Pin the role's Borsh tag to 0 and `TrustedUpdater` to 1. Contract JSON continues
   to serialize `"ModifyRoles"` and additionally accepts `"Admin"`; gateway JSON continues to
   serialize `"modify_roles"` and additionally accepts `"admin"`. Keep the existing canonical
   schemas, including their legacy enum spellings; the new spellings are serde input aliases.
   The manager advertises `admin` and continues to accept `modify-roles`. Internal authorization
   and new documentation use Admin. These are two names for one capability, not two roles.
8. Keep the interface controller-neutral. Do not extend the proxy-oracle governance contract,
   prescribe its use, or add proposal/policy scenarios for that hypothetical integration.

## Alternatives

- Partial patches preserve unmentioned live settings, but introduce another input type, merge
  semantics, and coupled validation paths. Full replacement keeps the interface smaller at the
  explicit cost of reviewing complete snapshots and execution order.
- Separate Owner authority can distinguish role operators from code administrators, but no such
  trust-tier requirement exists here. It adds storage, bootstrap and handover complexity. Admin
  deliberately gives the existing role administrators configuration and arbitrary-code powers.
- Renaming the Rust variant and adding only a serde alias preserves some inputs but breaks Rust
  variant imports. Emitting new canonical JSON also breaks newer tooling against the live old
  contract. Keeping legacy identities with an additive Admin alias preserves both directions.
- Keeping the handwritten bootstrap avoids adopting the standard state wrapper, but leaves a
  second migration protocol without version checks. The existing framework already supports
  missing version metadata as V0; use its transformer and entrypoint instead.
- Selecting a DAO or the proxy-oracle governance contract now would prove one deployment topology,
  but there is no current requirement to choose it. Test the adapter boundary directly and review
  a controller integration when one is actually selected.

## Consequences

Every Admin, including a caller granted the legacy `ModifyRoles` spelling, is trusted to replace
policies and deploy arbitrary code. This is an intentional expansion of that existing role's
authority, not merely a cosmetic rename. There is no timelock or code allowlist. Removing the last
Admin with the existing explicit override can disable administration on a keyless account;
changing roles does not remove full-access keys.

Acceptance requires:

- Fast tests for Admin/deposit boundaries, validation, atomic rejection, full replacement and
  unchanged cached/replay state. TrustedUpdater alone must not configure, upgrade or modify roles.
- Compatibility coverage for both serde spellings, unchanged canonical JSON and schemas, role
  bytes 0/1, legacy Rust enum imports/matches, and both manager CLI spellings. A new gateway must
  still address the old 0.1.0 contract using the legacy role spelling.
- Focused direct-WASM tests for released 0.1.0 V0-to-V1 migration and failure rollback, including
  configuration, feeds/timestamps, role memberships and the stored version. The old administrator
  must acquire Admin powers through the preserved role identity, with no new grant or owner.
- Generated migration checks for private access, invalid/stale/incomplete chains and current-state
  empty-input behavior; Admin-driven inline/global upgrades must preserve data and version state.
  Do not replace the removed governance fixture with another controller framework.
- Maximum-policy logging remains complete within the receipt log limit, with no configuration
  event advertised by a rejected call.
- Gateway and manager changes are limited to compatible role naming; add no gateway methods.
  Make no production governance, dependency-version or toolchain changes.

The useful local checks are `just test-fast -p templar-common --lib`,
`just test-fast -p templar-redstone-adapter-contract`, the affected gateway/manager fast tests and
gateway catalog check, and `just test-sandbox -p templar-gateway-testing --test redstone_admin`.
Also run a workspace check, adapter WASM check, affected-target lint, formatting and documentation
checks. Testnet and production execution are not authorized by this decision; verify target-network
limits, actual Admin memberships, external event consumers and monitoring rules before a separately
approved rollout.
An emitted event provides observability, not an installed alert.

| Failure scenario | Warning and prevention | Detection and recovery |
|---|---|---|
| An old full snapshot restores stale settings | Review every field and execution order, not only the intended difference | Compare `get_config` and successful `config_updated` records; submit a fresh complete configuration through Admin |
| Role identity changes or the last Admin is removed | Pin historical role bytes, test both name spellings, and verify Admin memberships before any separately approved key removal | Inspect `has_role`/`list_role`; retain deployment authority for reviewed recovery before key removal, otherwise recovery may be impossible |
| Deployment succeeds but migration fails or loses legacy data | Use the standard complete-chain validation and one deploy/migrate self-receipt; exercise the exact released binary and retained collection prefixes | Inspect all receipt failures, code identity, versions and configuration/role/feed snapshots; a failed receipt preserves the prior state and code |
| A compatibility alias changes outbound wire values | Preserve canonical legacy serialization, schema and Rust variant identities; test a new client against the legacy contract | Legacy decode and CLI/Rust compatibility probes identify regressions before release; correct the alias rather than adding version-dependent RPC branches |

Future state-layout changes increment the target version and use the shared migration chain.
A future controller integration and deployment choices belong in their own review.
