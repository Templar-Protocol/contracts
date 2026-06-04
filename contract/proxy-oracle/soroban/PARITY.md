# NEAR ↔ Soroban Proxy Oracle Parity

Behavioral parity baseline between the NEAR and Soroban proxy-oracle
implementations. Parity is at the semantic/outcome level — Soroban events are
compact typed events, not byte-for-byte equal to NEAR's JSON.

Baseline: `64bf8b821cabbc94e4591ca89997c8ec00f365c7`. Shared logic lives in the
`no_std` `templar-proxy-oracle-kernel` (aggregation, freshness, breakers) and
`templar-proxy-oracle-governance-kernel` (proposal lifecycle); each runtime
supplies its own storage, RBAC, and events.

| Feature | NEAR | Soroban | Notes |
| :--- | :--- | :--- | :--- |
| SEP-40 read surface | `list_ema_prices_no_older_than` | `aggregated_latest` / `aggregated_history` (normalized); SEP-40 served by per-feed `Sep40Adapter` | Adapter rescales to its own `decimals`; fail-closed when stale/missing |
| Source IO / aggregation | async cross-contract calls | synchronous within `refresh` | Quorum-based median; identical kernel |
| Freshness | `FreshnessFilter` | `FreshnessFilter` in `ProxyConfig` | Reject sources older than `max_age_secs`; identical kernel |
| Accepted vs observed history | `CircuitBreakerSet` history | `History(asset)` + `Breakers(asset)` | Rule baseline vs audit trail kept separate |
| `refresh` | `update_prices` (async callback) | `refresh` (sync) | Atomic cache + breaker update |
| Quorum failure | `min_sources` check | `min_sources` check | `refresh` fails if accepted sources < `min_sources` |
| Source base mismatch | implicit in request id | explicit `source_base` check | Reject a source whose `base()` ≠ proxy base |
| Manual trip | `set_circuit_breaker_manual_trip`, `ManualTripper` role, 1024-byte event metadata | `SetManualTrip(asset, tripped, metadata)`, `ManualTripper` role, 1024-byte event metadata | Neither action carries an operator id; NEAR records the predecessor (governance account) in-kernel, Soroban's event has none — correlate via the governance proposal |
| Breaker trip | kernel-driven | kernel-driven | Identical kernel; Soroban stores compact postcard state |
| Breaker config / rearm / enforce | `gov_create` + `gov_execute` | governed `AddBreaker` / `Rearm` / `SetEnforced` | `Rearm` and `SetEnforced` carry independent per-op TTLs |
| Cache invalidation | epoch bump | explicit cache removal on config change | Stale updates cannot write to a new config |
| TTL extension | N/A (permanent storage) | `extend_ttl` | Soroban-specific eviction guard |
| Governance lifecycle | `gov_create` / `gov_execute` | `create_proposal` / `execute_proposal` / `cancel_proposal` | Maturity-gated by id (no FIFO); per-op TTLs; 64-pending cap; queried via `active_ids` / `get_proposal` |
| Roles / authority | `Role` enum + `near-sdk-contract-tools::Rbac` | same 4 roles + OpenZeppelin `stellar-access` | Admin override; last-Admin removal rejected; the kernel does no role checks |
| Upgrade | versioned state | `upgrade(new_wasm_hash, operator)` + Admin `Upgrade` action | Typed surface; NEAR `AdminFunctionCall` dynamic dispatch intentionally not ported |
| Events | JSON | compact typed | Semantic, not byte parity |
| Asset lifecycle | `set_proxy(id, None)` | `add_asset` / `remove_asset` | Soroban keeps an explicit `Assets` list |
| Dedup / result shape | request-id dedup / map | `refresh` target dedup → `Vec<(Asset, RefreshStatus)>` | First-seen, deterministic |

Verify the shared kernel and both Soroban runtimes:

```bash
cargo test -p templar-proxy-oracle-kernel --features serde --lib
cargo test -p templar-proxy-oracle-soroban-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-governance-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-sep40-adapter-contract --features testutils
```
