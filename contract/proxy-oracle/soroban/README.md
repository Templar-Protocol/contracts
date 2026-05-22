# Soroban Proxy Oracle

Soroban proxy oracle contract for SEP-40-compatible price feeds.

The contract exposes SEP-40 cached reads (`base`, `assets`, `decimals`, `resolution`, `price`, `prices`, `lastprice`) and a separate `refresh(assets)` method. `refresh` reads configured SEP-40 source oracle contracts, aggregates source prices through `templar-proxy-oracle-kernel`, applies freshness filters and circuit breakers, and writes the accepted or failed result to cache.

RedStone integration is via RedStone's deployed Stellar SEP-40 wrapper contracts, not by writing RedStone prices in this proxy. RedStone payload verification and price reporting remain owned by the RedStone adapter/wrapper contracts.

Reads fail closed: `lastprice` returns `None` unless the latest cached status is accepted and still fresh under the proxy configuration.

Governance is handled by the companion `templar-proxy-oracle-soroban-governance-contract`. The runtime stores a governance address and all proxy/circuit-breaker mutations require that address to authorize the call. Governance proposals are queued with an action TTL in nanoseconds and must be accepted in proposal-id order after maturity, matching the NEAR proxy-oracle governance model.

## Operational Notes

- Configure at least one source and set `min_sources` between `1` and the number of configured sources. Invalid quorum settings are rejected.
- Use `refresh(assets)` as the only source IO path. SEP-40 reads are storage-only and never call source contracts.
- Manage circuit breakers through the runtime's generic `add_breaker(asset, config)`, `update_breaker(asset, breaker_id, update)`, and `remove_breaker(asset, breaker_id)` methods.
- Submit governance proposals through `submit(caller, action)`. The companion governance contract intentionally does not expose action-specific `submit_*` methods or per-kind accept/revoke lanes.
- Call `extend_ttl()` periodically on the runtime and governance contracts to preserve persistent and instance storage.
- Keep optimized runtime and governance WASM artifacts at or below `128 KiB` (`131072` bytes). Recheck size after runtime, governance, ABI, or event changes.

## Known Limits

- Source contracts must expose the SEP-40-compatible ABI used here. RedStone support is expected through RedStone Stellar SEP-40 wrapper contracts.
- The Soroban runtime does not implement NEAR Pyth sources or NEAR price transformers.
- Manual trip metadata is not stored on Soroban; manual trips are represented as a compact breaker status.
- Governance uses one `action_ttl_ns` for all proposal kinds, matching NEAR proxy-oracle governance rather than the vault per-kind timelock model.
- Events are compact Soroban events and are not byte-for-byte equivalent to NEAR proxy-oracle JSON events.

## Verification

- `cargo test -p templar-proxy-oracle-kernel --features serde --lib -- --nocapture`
- `cargo test -p templar-proxy-oracle-soroban-contract -- --nocapture`
- `cargo test -p templar-proxy-oracle-soroban-governance-contract -- --nocapture`
- `cargo build --profile release-soroban --target wasm32-unknown-unknown -p templar-proxy-oracle-soroban-contract`
- `cargo build --profile release-soroban --target wasm32-unknown-unknown -p templar-proxy-oracle-soroban-governance-contract`
- `stellar contract optimize --wasm target/wasm32-unknown-unknown/release-soroban/templar_proxy_oracle_soroban_contract.wasm --wasm-out target/wasm32-unknown-unknown/release-soroban/templar_proxy_oracle_soroban_contract.optimized.wasm`
- `stellar contract optimize --wasm target/wasm32-unknown-unknown/release-soroban/templar_proxy_oracle_soroban_governance_contract.wasm --wasm-out target/wasm32-unknown-unknown/release-soroban/templar_proxy_oracle_soroban_governance_contract.optimized.wasm`
