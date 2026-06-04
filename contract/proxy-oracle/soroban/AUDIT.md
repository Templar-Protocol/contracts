# Proxy Oracle Audit Boundary

Audit boundary, safety invariants, threat-model assumptions, and non-goals for
the Soroban proxy oracle. See `README.md` for the contract overview, `PARITY.md`
for NEAR parity, and `RUNBOOK.md` for operations.

## In scope

1. **Runtime** (`contract/src/lib.rs`) — normalized read API (`aggregated_latest`,
   `aggregated_history`), cache management and fail-closed reads, source IO and
   kernel integration via `refresh`, storage TTL (`extend_ttl`), governed manual
   trip via the `ManualTripper` role, and compact typed events on every
   state-change path. The runtime does **not** implement SEP-40.
2. **Governance** (`governance-contract/src/lib.rs`) — `create_proposal` with
   per-operation TTLs (`OperationKind` / `TtlConfig`) and a 64-pending cap;
   id-based `execute_proposal` (no FIFO) and `cancel_proposal`; timelock via
   `effective_ttl` (distinct `Rearm` / `SetEnforced` TTLs); `Address::require_auth`
   authorization; `SetRole` role management; last-Admin removal rejection;
   `Upgrade(new_wasm_hash)` for governed runtime upgrades; query views
   `active_ids`, `get_proposal`, `get_operation_ttl`, `get_effective_proposal_ttl`,
   `next_proposal_id`.
3. **SEP-40 adapter** (`sep40-adapter-contract/src/lib.rs`) — SEP-40
   `PriceFeedTrait`, declaring `contractmeta!(key = "sep", val = "40")`; reads the
   parent via `aggregated_latest` / `aggregated_history` and rescales to the
   adapter's `decimals`; owner-gated `set_metadata` / `upgrade` via
   `stellar_access::ownable` (two-step transfer). No on-chain decommission state.
4. **Shared DTOs** (`common/src/lib.rs`) and the **kernel**
   (`templar-proxy-oracle-kernel`: `MedianLow` aggregation, `FreshnessFilter`,
   and the `StepwiseChange` / `MonotonicRun` / `WindowedChangeDelta` breakers).

## Out of scope

Non-deployable support code (`justfile`, `scripts/`), Stellar CLI invocations,
RedStone's own Stellar SEP-40 wrapper contracts, off-chain keepers / refresh
bots, and monitoring infrastructure.

## Threat-model assumptions

- The Stellar network and Soroban host are trusted; host-level exploits are out
  of scope.
- The governance owner key is a secure multisig/process outside this boundary.
- RedStone wrapper contracts report correct prices and timestamps.
- Ledger timestamps are accurate within Soroban's resolution; extreme clock skew
  is out of model.
- An off-chain keeper calls `extend_ttl` at least weekly; eviction from missed
  TTL calls is an operational risk, not a contract bug.
- Deploy/upgrade tooling runs in a trusted environment with no hostile inputs.

## Safety topics

**Oracle manipulation.** `set_proxy` rejects `min_sources == 0`,
`min_sources > sources.len()`, empty source lists, and duplicate `(oracle, asset)`
pairs, so governance cannot install a zero-quorum config. `FreshnessFilter`
(enforced in `refresh_one` via `source_kernel_price`) drops stale sources before
aggregation. Breakers require governance proposals to add/update/remove; manual
trips require the `ManualTripper` role. `refresh_one` fails closed on missing storage keys
(`RESOLVE_FAILED_STORAGE_CODE`); `aggregated_latest` and adapter `lastprice`
return `None` on missing config.

**Governance and authorization.** Every runtime state-change except `refresh`
requires owner (`#[only_owner]`) authorization. Proposals are created and
executed by id after maturity; the 64-pending cap bounds vector growth.
Execution cannot precede the per-kind TTL (`effective_ttl` = max of requested
and configured minimum). `SetActionTtl` requires `ProxyConfigurationManager`
(Admin overrides). Revoking the last `Admin` is rejected (`LastAdmin`).
Ownership handoff emits `OwnershipTransferSubmitted` on submit and completes when
the new owner calls `accept_ownership`; monitoring should alert on ownership
transfer immediately.

**Storage and resources.** `extend_ttl` guards every potentially-absent key with
`storage.has` before extending and emits `TtlExtended`. Optimized WASM stays
within budget (runtime/governance ≤ 128 KiB, adapter ≤ 32 KiB), enforced by
`just size-check`. `refresh` is bounded by source count and the deduplicated
asset list; breaker evaluation is bounded by history length (≤ 32) and breaker
count (≤ 16 per asset).

**Operational.** Reads fail closed — `aggregated_latest` / adapter `lastprice`
return `None` on missing config, non-`Accepted` status, or staleness, with no
default fallback. Manual trips block the feed immediately and invalidate the
cache; metadata is event-only (≤ 1024 bytes). Any governance mutation to proxy
or breaker config clears the cached price (the explicit-removal analogue of
NEAR's stale-epoch handling).

## Known limitations and non-goals

- **Behavioral, not byte, parity** — Soroban events are compact typed XDR, not
  NEAR JSON. Verified at the outcome level (see `PARITY.md`).
- **RedStone dependency** — RedStone signature verification lives in RedStone's
  wrapper contracts, not here.
- **TTL liveness** — Soroban storage is not permanent; `extend_ttl` must run on a
  cadence (see `RUNBOOK.md`).
- **No new aggregation** — the kernel is shared with NEAR; no new algorithms.
- **No implicit migration** — earlier prototype storage layouts need an explicit
  migration or a reinitialized contract.
- **No `AdminFunctionCall`** — NEAR's arbitrary dynamic dispatch is intentionally
  not ported; the upgrade surface is the typed `upgrade` / `Upgrade` path.
- **Synchronous refresh** — all source IO is within one `refresh` transaction.
- **Budget scope** — full Stellar CPU/memory simulation needs a live RPC; the
  local `budget-check` runs deterministic soroban-sdk testutils scenarios.

## Verification

```bash
cargo test -p templar-proxy-oracle-kernel --features serde --lib
cargo test -p templar-proxy-oracle-soroban-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-governance-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-sep40-adapter-contract --features testutils
just -f contract/proxy-oracle/soroban/justfile audit-ready   # full gate
```
