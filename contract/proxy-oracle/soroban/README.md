# Soroban Proxy Oracle

Soroban proxy oracle that aggregates external SEP-40 price feeds into a normalized exponent-form cache. A companion **`Sep40Adapter`** contract re-exposes the proxy oracle's normalized prices as SEP-40 `PriceFeedTrait` for downstream consumers, with per-adapter `decimals` / `resolution` / `base`.

The runtime contract exposes:

- `refresh(assets)` — pulls source SEP-40 prices, aggregates through `templar-proxy-oracle-kernel`, applies freshness + breakers, writes accepted/failed status to cache.
- `aggregated_latest(asset)` — returns the cached `NormalizedPrice { mantissa, expo, timestamp }` if accepted and still fresh under the proxy configuration.
- `aggregated_history(asset, records)` — last N normalized cached prices for an asset.
- `registered_assets()`, `source_base()`, `get_proxy(asset)`, `get_cached(asset)`, `get_breaker_set_view(asset)`, `get_owner()` — admin / introspection helpers. The names are deliberately distinct from SEP-40's `assets()` / `base()`: `source_base()` is the source-validation invariant (every source must report against it), and `registered_assets()` is the enumeration of assets with registered proxy configs. Per-feed SEP-40 `base` and `assets` are declared on each `Sep40Adapter`. `get_owner()` comes from the standard `stellar_access::ownable::Ownable` trait.

The runtime contract does **not** implement SEP-40 itself. Per-feed SEP-40 exposure is the job of `Sep40Adapter` contracts — see `sep40-adapter-contract/`. Each adapter stores `(owner, parent_oracle, asset, decimals, resolution, base)` and dispatches SEP-40 reads to the parent's `aggregated_latest`. This lets different feeds publish at different decimals without forcing the proxy oracle to pick a single contract-wide precision.

RedStone integration is via RedStone's deployed Stellar SEP-40 wrapper contracts, not by writing RedStone prices in this proxy. RedStone payload verification and price reporting remain owned by the RedStone adapter/wrapper contracts.

Reads fail closed: `aggregated_latest` (on the runtime) and `lastprice` (on adapters) return `None` unless the latest cached status is accepted and still fresh under the proxy configuration.

Governance is handled by the companion `templar-proxy-oracle-soroban-governance-contract`. Ownership of the runtime contract is managed by `stellar_access::ownable` — the standard `Ownable` trait stores the current owner and exposes `get_owner`, `transfer_ownership` (two-step), `accept_ownership`, and `renounce_ownership`. Every proxy / circuit-breaker configuration mutation is gated with `#[only_owner]`, so it requires the owner address (typically the governance contract) to authorize the call. Emergency manual trip/untrip calls are authorized through the `ManualTripper` governance role via `SetManualTrip` proposals. Soroban intentionally omits NEAR `AdminFunctionCall` and keeps the upgrade surface typed through the governance contract's `Upgrade` action.

Handing ownership off to a new governance contract goes through the standard two-step transfer:

1. The current owner submits and accepts a `TransferOwnership(new_owner)` proposal. This dispatches to `transfer_ownership(new_owner, live_until_ledger)` on the runtime, with `live_until_ledger` set to the maximum allowed entry TTL window.
2. The new owner finalizes the handoff either by calling `accept_ownership()` directly on the runtime (if it's an EOA) or by submitting and accepting an `AcceptOwnership` proposal on its own governance contract (if it's another instance of `ProxyOracleGovernance`).

`RenounceOwnership` is also exposed as an Admin-only governance action. Accepting it dispatches to `renounce_ownership()` on the runtime and permanently removes the owner; any subsequent `#[only_owner]` call panics with `OwnableError::OwnerNotSet` and the proxy oracle's configuration becomes immutable. There is no undo.

The governance contract supports per-operation TTLs via `OperationKind` and `TtlConfig`. Each action kind has its own maturity delay, configurable through `SetActionTtl(kind, new_ttl_ns)`. Breaker lifecycle proposal actions are explicit: `Rearm` and `SetEnforced` each have their own TTL. The constructor seeds a uniform TTL across all kinds. Governance roles are `Admin`, `ManualTripper`, `CircuitBreakerOperator`, and `ProxyConfigurationManager`, with Admin able to override any action. `SetActionTtl` requires `ProxyConfigurationManager`. Removing the last Admin is rejected.

Proposals use `create_proposal(caller, id, operation, requested_ttl)` with id-based execution after maturity. At most 64 proposals may be pending at once; canceling or executing a proposal frees a slot. The `submit`/`accept`/`revoke` methods remain as compatibility aliases that delegate to the typed lifecycle. Mature proposals execute by id; no FIFO ordering is required. Query views include `next_proposal_id`, `active_ids`, `get_proposal`, `get_effective_proposal_ttl`, and `get_operation_ttl`. Callers derive count and membership from `active_ids` directly (`.len()`, `.contains(&id)`) — separate count / list-paginated views were removed to avoid duplicating the same information at the 64-proposal cap.

The runtime exposes a governed `upgrade(new_wasm_hash)` method for WASM upgrades. The governance contract provides `Upgrade(new_wasm_hash)` as the Admin-only proposal path. NEAR `AdminFunctionCall` arbitrary dynamic dispatch is intentionally not implemented on Soroban because it is not a safe typed parity surface.

SEP-40 metadata (`contractmeta!(key = "sep", val = "40")`) is declared on `Sep40Adapter`, not on the runtime contract.

## Sep40Adapter

Each adapter is independently owned (via `stellar-access`'s `Ownable` two-step transfer pattern) and tracks a single `(parent_oracle, asset)` pairing. Admin entrypoints, all owner-gated:

- `set_metadata(decimals, resolution, base)` — replace the owner-mutable SEP-40 metadata triple in a single call; emits `MetadataUpdated { decimals, resolution, base }`. `parent_oracle` and `asset` are immutable post-construction — repointing an adapter at a different parent or asset would silently invalidate downstream consumers, and the correct response is to deploy a new adapter.
- `config() -> Option<Config>` — single getter returning the entire `Config { parent_oracle, asset, decimals, resolution, base }` struct. Replaces the per-field getter views; the SEP-40 `PriceFeedTrait` reads (`base`, `assets`, `decimals`, `resolution`, `price`, `prices`, `lastprice`) remain as required by the trait.
- `upgrade(BytesN<32>)` — swap the adapter wasm.
- `transfer_ownership` / `accept_ownership` / `renounce_ownership` — from the `Ownable` trait.

Decommissioning is out of band: no special "deactivated" state on-chain. Owners upgrade to a no-op wasm, renounce ownership, transfer ownership to a burn address, or simply stop publishing the address. The proxy oracle has no registry of adapters — anyone can deploy an adapter pointing at the proxy oracle, and Templar's "official" adapters are listed in the release manifest / docs.

## Operational Notes

- Configure at least one source and set `min_sources` between `1` and the number of configured sources. Invalid quorum settings are rejected.
- Use `refresh(assets)` as the only source IO path. `aggregated_latest` (runtime) and SEP-40 reads (adapters) are storage-only and never call source contracts.
- Manage circuit breakers through the runtime's `add_breaker(asset, config)`, `remove_breaker(asset, breaker_id)`, `rearm(asset, breaker_id, config)`, and `set_enforced(asset, breaker_id, config)` methods.
- Grant emergency operators with `SetRole(account, ManualTripper, true)`. `GovernanceAction::SetManualTrip(actor, asset, is_manually_tripped, metadata)` retains the actor for event attribution, and the actor must match the authenticated proposal creator.
- Manual-trip metadata is event-only, capped at 1024 bytes, and not stored in breaker state.
- Submit governance proposals through `submit(caller, action)` (compatibility alias) or `create_proposal(caller, id, operation, requested_ttl)`. The companion governance contract intentionally does not expose action-specific `submit_*` methods or per-kind accept/revoke lanes.
- Per-operation TTLs are configurable via `SetActionTtl(kind, new_ttl_ns)`. Query current TTLs with `get_operation_ttl(kind)` or `get_effective_proposal_ttl(operation, requested_ttl)`. Use explicit `Rearm` and `SetEnforced` proposal actions for breaker lifecycle timing.
- Manage governance roles with `SetRole(account, role, set)` for `Admin`, `ManualTripper`, `CircuitBreakerOperator`, `ProxyConfigurationManager`.
- Runtime upgrades use `upgrade(new_wasm_hash)` authorized by governance. Governance proposes the upgrade via the `Upgrade(new_wasm_hash)` action. NEAR `AdminFunctionCall` is intentionally not implemented on Soroban.
- Call `extend_ttl()` periodically on the runtime and governance contracts to preserve persistent and instance storage.
- Keep optimized runtime, governance, and adapter WASM artifacts within their size budgets: runtime and governance at or below `128 KiB` (`131072` bytes); adapter at or below `32 KiB` (`32768` bytes). Recheck sizes after ABI or event changes; release and audit gates write supporting evidence under `.omo/evidence`.

## Known Limits

- Source contracts must expose the SEP-40-compatible ABI used here. RedStone support is expected through RedStone Stellar SEP-40 wrapper contracts.
- The Soroban runtime does not implement NEAR Pyth sources or NEAR price transformers.
- Manual trip metadata is not stored on Soroban; bounded metadata is emitted in the compact manual-trip event and manual trips are represented in state as a compact breaker status.
- Governance uses per-operation TTLs via `OperationKind` / `TtlConfig`, not a single runtime TTL. The constructor `initial_uniform_ttl_ns` seeds the same TTL on every operation kind. `SetActionTtl(kind, new_ttl_ns)` changes the TTL for a specific operation kind afterward, with distinct `Rearm` and `SetEnforced` TTLs for breaker lifecycle actions.
- Proposals execute by id after maturity; no FIFO ordering is required. `submit`/`accept`/`revoke` remain as compatibility aliases.
- This governance model is not an implicit in-place migration for earlier prototype Soroban storage layouts. Existing deployments that stored different role, TTL, or pending-proposal keys need an explicit migration or a redeployed/reinitialized governance contract.
- NEAR `AdminFunctionCall` arbitrary dynamic dispatch is intentionally not implemented on Soroban. The upgrade surface is typed: `upgrade(new_wasm_hash)` on runtime, `Upgrade(new_wasm_hash)` via governance.
- Events are compact Soroban events and are not byte-for-byte equivalent to NEAR proxy-oracle JSON events.
- Governance proposal and role state-machine logic is shared with NEAR through `templar-proxy-oracle-governance-kernel`; Soroban still owns authorization, storage encoding, events, and runtime dispatch. Generated typed clients are not implemented in this slice; the current explicit `authorize_as_current_contract` pattern with exact `ContractContext` fn_name and args remains acceptable for this contract scope.
- OpenZeppelin `stellar-contract-utils::upgradeable` is intentionally NOT adopted: crates.io v0.7.1 uses `is_multiple_of` (stabilized in Rust 1.87) in its unconditionally-compiled `crypto::merkle` module, but `rust-toolchain.toml` pins us to 1.86.0. The Templar `templar/rust-1.86-compat` fork backports the API to 1.86 but is locked to soroban-sdk 23.x, incompatible with the 25.0.1 we use here. The hand-rolled `upgrade(BytesN<32>)` on the runtime and adapter contracts is the deliberate stopgap until either (a) the workspace toolchain bumps to ≥ 1.87 or (b) the fork rebases onto soroban-sdk 25. Do not re-investigate without one of those changing first.
- OpenZeppelin `stellar-access::access_control` is already in use under `governance-contract/src/roles.rs`. That file is a typed wrapper (`Role` enum → `Symbol`) over `grant_role_no_auth` / `revoke_role_no_auth` / `has_role` / `get_role_member*`, plus a last-`Admin` guard. The standard `role_granted` / `role_revoked` events are already emitted by the library. Exposing the `AccessControl` trait directly on the contract's ABI was considered and rejected: its default mutating methods (`grant_role`, `revoke_role`, `renounce_role`, `transfer_admin_role`, `accept_admin_transfer`, `set_role_admin`, `renounce_admin`) would bypass the proposal flow unless each is overridden to panic, which is more code than the standardization is worth. Don't re-litigate.

## Verification

- `cargo test -p templar-proxy-oracle-kernel --features serde --lib -- --nocapture`
- `cargo test -p templar-proxy-oracle-soroban-contract --features testutils -- --nocapture`
- `cargo test -p templar-proxy-oracle-soroban-governance-contract --features testutils -- --nocapture`
- `cargo test -p templar-proxy-oracle-soroban-sep40-adapter-contract --features testutils -- --nocapture`
- `just -f contract/proxy-oracle/soroban/justfile build` — builds runtime, governance, and adapter unoptimized WASMs.
- `just -f contract/proxy-oracle/soroban/justfile optimize` — builds + optimizes all three to `target/proxy-oracle-soroban/wasm/*.optimized.wasm`.

All three contracts must build via `stellar contract build` (not plain `cargo build`): the governance contract and the adapter both depend on `stellar-access`, which enables soroban-sdk's `experimental_spec_shaking_v2` — a feature that only resolves under the Stellar CLI (v25.2.0+).
