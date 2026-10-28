# NEAR ↔ Soroban Proxy Oracle Parity

Behavioral parity baseline between the NEAR and Soroban proxy-oracle
implementations. Parity is at the semantic/outcome level — Soroban events are
compact typed events, not byte-for-byte equal to NEAR's JSON.

Historical baseline: `64bf8b821cabbc94e4591ca89997c8ec00f365c7`; the table describes
the new-stack boundary, not verification of that revision or a live upgrade.
Shared logic lives in the
`no_std` `templar-proxy-oracle-kernel` (aggregation, freshness, breakers) and
`templar-proxy-oracle-governance-kernel` (proposal lifecycle); each runtime
supplies its own storage, RBAC, and events.

| Feature | NEAR | Soroban | Notes |
| :--- | :--- | :--- | :--- |
| SEP-40 read surface | `list_ema_prices_no_older_than` | `aggregated_latest` / `aggregated_history` (normalized); SEP-40 served by per-feed `Sep40Adapter` | Adapter rescales to its own `decimals`; fail-closed when stale/missing |
| Source IO / aggregation | async cross-contract calls; weighted medians / Priority | synchronous `refresh`; equal-weight MedianLow / MedianHigh / Priority | Shared kernel; Priority is first surviving source, no quorum |
| Pyth Lazer source | `Lazer` source kind reads the NEAR adapter by feed id | `PythLazerSource` is a SEP-40 contract keyed `Other("<feed id>")`, listed in `SetProxy` like any source | Both push→persist→serve; Soroban delegates signature checks to Pyth's on-chain verifier and keeps freshness + anti-replay |
| Maintenance fan-out | N/A (one call refreshes all sources) | `ProxyOracleBatcher` batches `refresh` / `extend_ttl` per asset | Stellar allows one Soroban op per transaction |
| Freshness | `FreshnessFilter` | required `u64` age/drift converted to kernel freshness | Zero age allowed; latest reads check cache and publication age inclusively |
| Accepted vs observed history | `CircuitBreakerSet` history | `History(asset)` + `Breakers(asset)` | Rule baseline vs audit trail kept separate |
| `refresh` | `update_prices` (async callback) | `refresh` (sync) | Atomic cache + breaker update |
| Quorum failure | median `min_sources` check | `MedianConfig.min_sources` only | Priority has no quorum; all-invalid fails closed |
| Source base / scale | source-descriptor-specific IO | `source_base` + explicit `expected_decimals` pins | Admission checks live scale; refresh discards drift; identity is not provider independence |
| Manual trip | `set_circuit_breaker_manual_trip`, `ManualTripper` role, 1024-byte event metadata | `SetManualTrip(asset, tripped, metadata)`, `ManualTripper` role, 1024-byte event metadata | Neither action carries an operator id; NEAR records the predecessor (governance account) in-kernel, Soroban's event has none — correlate via the governance proposal |
| Breaker trip | kernel-driven | kernel-driven | Identical kernel; Soroban stores compact postcard state |
| Breaker config / rearm / enforce | `gov_create` + `gov_execute` | governed `AddBreaker` / `Rearm` / `SetEnforced` | `Rearm` and `SetEnforced` carry independent per-op TTLs |
| Cache invalidation | epoch bump | explicit cache removal on config change | Stale updates cannot write to a new config |
| TTL extension | N/A (permanent storage) | `extend_ttl` | Soroban-specific eviction guard |
| Governance lifecycle | `gov_create` / `gov_execute` | `create_proposal` / `execute_proposal` / `cancel_proposal` | Maturity-gated by id (no FIFO); per-op TTLs; 64-pending cap; queried via `active_ids` / `get_proposal` |
| Roles / authority | `Role` enum + `near-sdk-contract-tools::Rbac` | same 4 roles + OpenZeppelin `stellar-access` | Admin override; last-Admin removal rejected; the kernel does no role checks |
| Upgrade | versioned state | typed code-swap surface, no governance self-upgrade/runtime migrate endpoint | This incompatible runtime/governance release is fresh-stack-only; in-place migration skipped |
| Events | JSON | compact typed | Semantic, not byte parity |
| Asset lifecycle | `set_proxy(id, None)` | `set_proxy` / `remove_proxy`, explicit `Assets` registry (≤ 64) | Reset/remove retains next-breaker-ID tombstone; no ID reuse or silent decoder reset |
| Refresh shape | one call, all sources, request-id dedup | `refresh(asset)` per asset → `RefreshStatus`; `refresh_many` via the batcher | First-seen, deterministic |

Soroban `ProxyConfig { aggregator, max_age_secs, max_clock_drift_secs }` uses
`AggregatorConfig::{MedianLow(MedianConfig), MedianHigh(MedianConfig),
Priority(Vec<SourceConfig>)}`; `MedianConfig { sources, min_sources }` and
`SourceConfig { oracle, asset, expected_decimals }` are the sole configuration
boundary. No weights or Priority quorum are exposed. Changed aggregation
method/order/pins/quorum resets breakers preserving their allocator; changed
freshness only retains breakers. Both clear cache/history; identical config is a
no-op. Adapter history projects newest-first distinct buckets, while parent
history stays chronological, freshness-independent, and breaker-gated.

Relative rules cannot create a trusted bootstrap sample. CumulativeChange
requires a fresh accepted installation baseline; rearm preserves it and history.
Window/streak rules count samples, so cadence/gaps matter. `ArmedAfter` is
intentional privileged recovery. Proposal maturity is snapshotted with no expiry;
cancellation is creator/current-Admin, execution current-role-authorized.
Explicit-ID retries retain exact payloads; unknown outcomes stop, not reselect.

The unified XDR and eight proposal groups of eight unchanged bodies are
incompatible with previous Soroban stacks. Leave them unchanged and deploy a
fresh matching runtime/governance. Adapter-only upgrade compatibility needs
exact old/new Wasm proof; `integration-tests/tests/upgrade.rs` does not prove a
successful swap. See [runbook](RUNBOOK.md) for wiring, timelock hardening, complete
role TTL paging, and deployment restrictions.

Verify the shared kernel and Soroban stack through the repository test
entrypoints:

```bash
just test-fast -p templar-proxy-oracle-kernel --features serde --lib
just -f contract/proxy-oracle/soroban/justfile test
just -f contract/proxy-oracle/soroban/justfile test-integration
```
