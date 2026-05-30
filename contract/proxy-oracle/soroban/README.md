# Soroban Proxy Oracle

Soroban proxy oracle contract for SEP-40-compatible price feeds.

The contract exposes SEP-40 cached reads (`base`, `assets`, `decimals`, `resolution`, `price`, `prices`, `lastprice`) and a separate `refresh(assets)` method. `refresh` deduplicates input assets in first-seen order, reads configured SEP-40 source oracle contracts once per target, aggregates source prices through `templar-proxy-oracle-kernel`, applies freshness filters and circuit breakers, and writes the accepted or failed result to cache.

`decimals()` is the oracle-wide SEP-40 output price precision, not the token decimal count for any individual asset. Every accepted `PriceData.price` returned by this contract is normalized to that shared precision (`price / 10^decimals`) regardless of the source feed precision. This supports one proxy oracle serving multiple feeds as long as all consumers agree on the contract-level output precision.

RedStone integration is via RedStone's deployed Stellar SEP-40 wrapper contracts, not by writing RedStone prices in this proxy. RedStone payload verification and price reporting remain owned by the RedStone adapter/wrapper contracts.

Reads fail closed: `lastprice` returns `None` unless the latest cached status is accepted and still fresh under the proxy configuration.

Governance is handled by the companion `templar-proxy-oracle-soroban-governance-contract`. The runtime stores a governance address and all proxy/circuit-breaker configuration mutations require that address to authorize the call. Emergency manual trip/untrip calls are authorized through the `ManualTripper` governance role via `SetManualTrip` proposals. Soroban intentionally omits NEAR `AdminFunctionCall` and keeps the upgrade surface typed through `AdminUpgrade`.

The governance contract supports per-operation TTLs via `OperationKind` and `TtlConfig`. Each action kind has its own maturity delay, configurable through `SetActionTtl(kind, new_ttl_ns)`. Breaker lifecycle proposal actions are explicit: `Rearm` and `SetEnforced` each have their own TTL. The constructor seeds a uniform TTL across all kinds. Governance roles are `Admin`, `ManualTripper`, `CircuitBreakerOperator`, and `ProxyConfigurationManager`, with Admin able to override any action. `SetActionTtl` requires `ProxyConfigurationManager`. Removing the last Admin is rejected.

Proposals use `create_proposal(caller, id, operation, requested_ttl)` with id-based execution after maturity. At most 64 proposals may be pending at once; canceling or executing a proposal frees a slot. The `submit`/`accept`/`revoke` methods remain as compatibility aliases that delegate to the typed lifecycle. Mature proposals execute by id; no FIFO ordering is required. Query views include `next_proposal_id`, `proposal_count`, `list_proposals`, `get_proposal`, `get_effective_proposal_ttl`, and `get_operation_ttl`.

The runtime exposes a governed `upgrade(new_wasm_hash)` method for WASM upgrades. The governance contract provides `AdminUpgrade(new_wasm_hash)` as the Admin-only proposal path. NEAR `AdminFunctionCall` arbitrary dynamic dispatch is intentionally not implemented on Soroban because it is not a safe typed parity surface.

The contract declares SEP-40 metadata via `contractmeta!(key = "sep", val = "40")`.

## Operational Notes

- Configure at least one source and set `min_sources` between `1` and the number of configured sources. Invalid quorum settings are rejected.
- Use `refresh(assets)` as the only source IO path. SEP-40 reads are storage-only and never call source contracts.
- Manage circuit breakers through the runtime's `add_breaker(asset, config)`, `remove_breaker(asset, breaker_id)`, `rearm(asset, breaker_id, config)`, and `set_enforced(asset, breaker_id, config)` methods.
- Grant emergency operators with `SetRole(account, ManualTripper, true)`. `GovernanceAction::SetManualTrip(actor, asset, is_manually_tripped, metadata)` retains the actor for event attribution, and the actor must match the authenticated proposal creator.
- Manual-trip metadata is event-only, capped at 1024 bytes, and not stored in breaker state.
- Submit governance proposals through `submit(caller, action)` (compatibility alias) or `create_proposal(caller, id, operation, requested_ttl)`. The companion governance contract intentionally does not expose action-specific `submit_*` methods or per-kind accept/revoke lanes.
- Per-operation TTLs are configurable via `SetActionTtl(kind, new_ttl_ns)`. Query current TTLs with `get_operation_ttl(kind)` or `get_effective_proposal_ttl(operation, requested_ttl)`. Use explicit `Rearm` and `SetEnforced` proposal actions for breaker lifecycle timing.
- Manage governance roles with `SetRole(account, role, set)` for `Admin`, `ManualTripper`, `CircuitBreakerOperator`, `ProxyConfigurationManager`.
- Runtime upgrades use `upgrade(new_wasm_hash)` authorized by governance. Governance upgrades use `AdminUpgrade(new_wasm_hash)` proposal. NEAR `AdminFunctionCall` is intentionally not implemented on Soroban.
- Call `extend_ttl()` periodically on the runtime and governance contracts to preserve persistent and instance storage.
- Keep optimized runtime and governance WASM artifacts at or below `128 KiB` (`131072` bytes). Latest release-gate sizes are runtime `121114` bytes (`118.28 KiB`) and governance `55409` bytes (`54.11 KiB`). Recheck size after runtime, governance, ABI, or event changes; release and audit gates write supporting evidence under `.omo/evidence`.

## Known Limits

- Source contracts must expose the SEP-40-compatible ABI used here. RedStone support is expected through RedStone Stellar SEP-40 wrapper contracts.
- The Soroban runtime does not implement NEAR Pyth sources or NEAR price transformers.
- Manual trip metadata is not stored on Soroban; bounded metadata is emitted in the compact manual-trip event and manual trips are represented in state as a compact breaker status.
- Governance uses per-operation TTLs via `OperationKind` / `TtlConfig`, not a single runtime TTL. The constructor `action_ttl_ns` seeds uniform TTLs. `SetActionTtl(kind, new_ttl_ns)` changes the TTL for a specific operation kind, with distinct `Rearm` and `SetEnforced` TTLs for breaker lifecycle actions.
- Proposals execute by id after maturity; no FIFO ordering is required. `submit`/`accept`/`revoke` remain as compatibility aliases.
- This governance model is not an implicit in-place migration for earlier prototype Soroban storage layouts. Existing deployments that stored different role, TTL, or pending-proposal keys need an explicit migration or a redeployed/reinitialized governance contract.
- NEAR `AdminFunctionCall` arbitrary dynamic dispatch is intentionally not implemented on Soroban. The upgrade surface is typed: `upgrade(new_wasm_hash)` on runtime, `AdminUpgrade(new_wasm_hash)` via governance.
- Events are compact Soroban events and are not byte-for-byte equivalent to NEAR proxy-oracle JSON events.
- Governance proposal and role state-machine logic is shared with NEAR through `templar-proxy-oracle-governance-kernel`; Soroban still owns authorization, storage encoding, events, and runtime dispatch. Generated typed clients are not implemented in this slice; the current explicit `authorize_as_current_contract` pattern with exact `ContractContext` fn_name and args remains acceptable for this contract scope.

## Verification

- `cargo test -p templar-proxy-oracle-kernel --features serde --lib -- --nocapture`
- `cargo test -p templar-proxy-oracle-soroban-contract -- --nocapture`
- `cargo test -p templar-proxy-oracle-soroban-governance-contract -- --nocapture`
- `just -f contract/proxy-oracle/soroban/justfile build` — builds both unoptimized WASMs.
- `just -f contract/proxy-oracle/soroban/justfile optimize` — builds + optimizes both WASMs to `target/proxy-oracle-soroban/wasm/*.optimized.wasm`.

Both contracts must build via `stellar contract build` (not plain `cargo build`): the governance contract depends on `stellar-access`, which enables soroban-sdk's `experimental_spec_shaking_v2` — a feature that only resolves under the Stellar CLI (v25.2.0+).
