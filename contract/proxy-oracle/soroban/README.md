# Soroban Proxy Oracle

Aggregates external SEP-40 price feeds into a normalized, exponent-form cache. A companion `Sep40Adapter` contract re-exposes the cached prices as SEP-40 `PriceFeedTrait` for downstream consumers at per-adapter `decimals` / `resolution` / `base`.

The runtime is **not** itself a SEP-40 contract. It exposes:

- `refresh(assets)` — pull source prices, aggregate through `templar-proxy-oracle-kernel`, apply freshness + breakers, write accepted/failed status to the cache. The only path that performs source IO.
- `aggregated_latest(asset) -> Option<NormalizedPrice>` — cached `{ mantissa, expo, timestamp }`, or `None` if not accepted or stale.
- `aggregated_history(asset, records)` — last N normalized cached prices.
- Introspection: `registered_assets`, `source_base`, `get_proxy`, `get_cached`, `get_breaker_set_view`, `get_owner`. Named to avoid colliding with SEP-40's `assets()` / `base()`: `source_base` is the validation invariant every source must report against; `registered_assets` enumerates assets with a proxy config.

Reads fail closed: `aggregated_latest` (runtime) and `lastprice` (adapter) return `None` unless the latest cached status is accepted and still fresh.

RedStone enters through RedStone's own Stellar SEP-40 wrapper contracts; this proxy does not verify RedStone payloads.

## Governance

The runtime's owner — normally the companion `templar-proxy-oracle-soroban-governance-contract` — is managed by `stellar_access::ownable` (two-step `transfer_ownership` / `accept_ownership` / `renounce_ownership`, plus `get_owner`). Every config mutation is `#[only_owner]`, so the owner must authorize it.

- **Handoff**: an Admin `TransferOwnership(new_owner)` proposal dispatches `transfer_ownership`; the new owner finalizes with `accept_ownership` (directly, or via an `AcceptOwnership` proposal on its own governance contract).
- **Renounce**: `RenounceOwnership` permanently clears the owner; every later `#[only_owner]` call then panics and the config is frozen. No undo.
- **Roles** (`stellar-access` RBAC): `Admin`, `ManualTripper`, `CircuitBreakerOperator`, `ProxyConfigurationManager`. Admin overrides any action; the last Admin cannot be removed. Emergency trips use the `SetManualTrip` action (ManualTripper role).
- **Proposals**: `create_proposal(caller, id, operation, requested_ttl)`, executed by id after maturity via `execute_proposal`; `cancel_proposal` frees a slot. At most 64 pending. Query with `active_ids`, `get_proposal`, `get_operation_ttl`, `get_effective_proposal_ttl`. Per-operation maturity (`OperationKind` / `TtlConfig`) is seeded uniform at construction and adjusted with `SetActionTtl`; `Rearm` and `SetEnforced` carry independent TTLs.
- **Upgrades**: `upgrade(new_wasm_hash, operator)` on the runtime, proposed via the Admin `Upgrade` action. NEAR's `AdminFunctionCall` arbitrary dispatch is intentionally not ported — the upgrade surface stays typed.

The proposal state machine is shared with NEAR via the `no_std` `templar-proxy-oracle-governance-kernel`; each runtime owns its own authorization, storage encoding, and events.

## Sep40Adapter

Each adapter is independently `Ownable` and tracks one **immutable** `(parent_oracle, asset)` pair — to repoint either, deploy a new adapter. Owner entrypoints:

- `set_metadata(decimals, resolution, base)` — replace the SEP-40 metadata triple; emits `MetadataUpdated`. `decimals ≤ 18`, `resolution ≠ 0`.
- `config() -> Option<Config>` — the full `{ parent_oracle, asset, decimals, resolution, base }`.
- `upgrade(new_wasm_hash, operator)` — owner-gated wasm swap.

SEP-40 `PriceFeedTrait` reads dispatch to the parent's `aggregated_latest` / `aggregated_history`, rescaled to the adapter's `decimals`. SEP-40 metadata (`contractmeta!(key = "sep", val = "40")`) is declared here, not on the runtime. There is no on-chain adapter registry or decommission state: owners upgrade to a no-op, renounce, transfer to a burn address, or stop publishing. Official adapters are listed in the release manifest.

## Operational notes

- Configure ≥ 1 source; `min_sources` must be in `[1, sources.len()]`. Invalid quorum is rejected.
- `refresh(assets)` is the only source-IO path; all reads are storage-only.
- Manage breakers with the governed `add_breaker` / `remove_breaker` / `rearm` / `set_enforced`. Inert params (zero thresholds/streaks/lookback) are rejected.
- Manual-trip metadata is event-only, capped at 1024 bytes, not stored in breaker state.
- Call `extend_ttl()` on the runtime and governance contracts on a cadence to avoid storage eviction.
- Keep optimized WASMs within budget: runtime & governance ≤ 128 KiB, adapter ≤ 32 KiB. Recheck after ABI/event changes.

## Known limits

- Source contracts must expose the SEP-40 ABI used here. NEAR Pyth sources and NEAR price transformers are not ported.
- Soroban storage is not permanent (unlike NEAR); a missed `extend_ttl` risks eviction. Events are compact typed events, not byte-for-byte equal to NEAR's JSON events.
- Not an in-place migration target for earlier prototype storage layouts — redeploy/reinitialize or ship an explicit migration first.
- **OZ `upgradeable` not adopted**: crates.io v0.7.1 needs Rust ≥ 1.87 (`is_multiple_of`) but the toolchain pins 1.86; the 1.86-compat fork is locked to soroban-sdk 23.x, not the 25.0.1 used here. The hand-rolled `upgrade` is the stopgap until the toolchain bumps or the fork rebases — don't re-investigate without one of those.

## Verification

```bash
cargo test -p templar-proxy-oracle-kernel --features serde --lib
cargo test -p templar-proxy-oracle-soroban-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-governance-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-sep40-adapter-contract --features testutils
just -f contract/proxy-oracle/soroban/justfile build     # unoptimized WASMs
just -f contract/proxy-oracle/soroban/justfile optimize   # optimized WASMs
```

All three contracts must build via `stellar contract build` (not plain `cargo build`): `stellar-access` enables soroban-sdk's `experimental_spec_shaking_v2`, which only resolves under the Stellar CLI (v25.2.0+).
