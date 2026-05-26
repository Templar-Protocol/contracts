# Proxy Oracle

The proxy oracle stores per-price proxy definitions on NEAR, resolves underlying Pyth/RedStone sources asynchronously, applies freshness filters, aggregates the surviving prices, gates the result through per-proxy circuit breakers, and caches the latest per-price update result.

## Directory Structure

- `kernel`: shared no-std proxy, aggregation, freshness, and circuit-breaker logic.
- `near/common`: NEAR DTOs, governance operations, source/request types, and versioned state.
- `near/contract`: deployable proxy oracle contract and callback/governance implementation.
- `near/lst-contract`: (legacy) LST adapter contract for transformed price feeds.

## Configuration

Each proxied price should be configured with independent sources, a freshness filter, and a circuit-breaker set. Prefer multiple independent sources for important feeds; configure `min_sources` so one compromised or stale source cannot determine the price alone.

Freshness filters are mandatory for production feeds. Circuit breakers only compare accepted observations; they do not protect against stale or future-dated source prices.

Circuit breaker accepted history must be large enough for every installed rule. A zero or too-small history is effectively disabled protection, even if breakers are installed, armed, and enforced.

Use complementary breaker types: `StepwiseChange` catches sudden jumps, while `MonotonicRun` and `WindowedChangeDelta` help catch staged ramps. Avoid inert parameters such as zero streaks, windows shorter than two observations, and zero lookback windows.

History length can be configured up to 32 entries, and at most 16 breakers may be configured per proxy. Recalibrate gas and storage before raising either bound.

## Operations

Proxy and circuit-breaker configuration changes are proposal-governed. Configure the proxy and breaker history before installing breakers, then add breakers with explicit monotonic IDs. Governance proposals may be created, executed, or cancelled by an account holding the role required for that operation; `Role::Admin` is the global governance role and may act on any proposal.

The NEAR governance contract is initialized as its own contract account with `new(proxy_oracle_id, admin_id, ttls)`. It seeds `admin_id` into RBAC as the initial `Role::Admin`; it is not an in-place migration target for legacy embedded proxy-oracle governance state.

`update_prices(price_ids)` performs oracle IO, aggregation, circuit-breaker evaluation, event emission, breaker-state persistence, and cache writes. `list_ema_prices_no_older_than(price_ids, age)` is a cached read only: it returns `None` when a cached result is missing, blocked, resolve-failed, or stale under the caller-provided `age`.

`update_prices` does not accept a caller freshness age. Governed proxy `FreshnessFilter` settings control source freshness during updates; caller freshness is applied only when reading accepted cached prices.

Enforcement and lifecycle are separate. Unenforced breakers still evaluate and can trip while the set has no existing blocking trip. Re-arming requires an explicit accepted-history source: empty history or observed history collected during the incident.

`get_proxy_circuit_breaker_set` exposes both `accepted_history` and `observed_history`. Accepted history is the rule baseline and only records non-blocking evaluations. Observed history records valid sampled prices even while the set is tripped or manually blocked, and should be treated as recovery/audit data until governance explicitly seeds from it.

Manual trip/untrip is available through governance operation `SetManualTrip { id, is_manually_tripped, metadata }` for offline incident response and requires `Role::ManualTripper`. Circuit-breaker lifecycle operations `Rearm` and `SetEnforced` require `Role::CircuitBreakerOperator`. Proxy definitions, circuit-breaker configuration, breaker add/remove, and TTL policy changes require `Role::ProxyConfigurationManager`. Governance roles are multi-role memberships managed with targeted `SetRole { account_id, role, set }` operations: `set: true` grants the named role and `set: false` revokes only that named role. `Role::Admin` is the global governance superuser role and may act on any proposal; removing the final `Role::Admin` membership is rejected.

Proxy oracle contract upgrades are available through the Admin-only `AdminUpgrade` operation, which dispatches the proxy's owner-gated `admin_upgrade` entrypoint. `AdminFunctionCall` is also Admin-only and dispatches a raw function call only to the governance contract's configured proxy oracle account; it is intended for exceptional proxy-admin actions such as accepting ownership after an owner transfer is proposed, not for arbitrary receiver calls. Its stored operation gas is a regular NEAR gas value; the manager CLI accepts either raw `--gas` units or `--tgas` shorthand.

Manual-trip metadata is event-only, encoded as `Base64VecU8`, capped at 1024 bytes, and not stored in contract state. Offline manual-trip events are emitted only when the manual-trip state changes. Governance-derived circuit-breaker configuration events are emitted for successful executions, except no-op manual-trip executions do not emit a manual-trip event.

Proxy and circuit-breaker changes clear the cached price and bump an internal per-price update epoch. In-flight update callbacks whose epoch no longer matches are ignored, so stale callbacks cannot repopulate cache or mutate breaker state after configuration changes.

Circuit-breaker events use the `templar-proxy-oracle` standard and names prefixed with `circuit_breaker_*`, including configuration, add/remove, enforcement, rearm, role, manual-trip, and automatic trip events. Automatic trip events include `is_enforced` so consumers can distinguish tripped-but-non-blocking breakers from blocking trips.

Off-chain services should use the proxy oracle path for protected feeds. Falling back to direct Pyth/Hermes reads bypasses proxy aggregation and circuit-breaker semantics. The relayer and liquidator update underlying oracle sources first, then call proxy `update_prices` for market-facing proxy price IDs before dependent actions. Operators running other flows must do the same on a cadence or before actions that require fresh proxy prices; cached reads fail closed until an accepted update is available.
