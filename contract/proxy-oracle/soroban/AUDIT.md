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
   `PriceFeedTrait`, declaring `contractmeta!(key = "sep", val = "40")`; binds
   immutable `(parent_oracle, asset, decimals, resolution, base)` constructor
   metadata, fails closed after decommission or parent-base drift, and exposes
   only owner-gated decommission/upgrade plus permissionless TTL maintenance.
4. **Pyth Lazer source** (`pyth-lazer-source-contract/src/lib.rs`) — SEP-40 source over
   Pyth's stateless verifier: channel filter, owner-maintained 64-feed registry
   sized for a full replacement within ledger-write limits, per-feed freshness
   window, per-feed strictly-advancing publish time
   (anti-replay), feeds served under their Lazer id (`Asset::Other("23")`), and
   explicit verification epochs that clear prices and restart the replay domain
   when the owner changes verifier/channel configuration. Price precision is
   constructor-bound.
5. **Batcher** (`batcher-contract/src/lib.rs`) — stateless fan-out of the runtime's
   permissionless `refresh` / `extend_ttl` and sibling `extend_ttl()` calls, plus
   instance-and-code TTL renewal for every target. Every entrypoint rejects more
   than 64 items before dispatch.
6. **Shared DTOs** (`common/src/lib.rs`) and the **kernel**
   (`templar-proxy-oracle-kernel`: `MedianLow` aggregation, `FreshnessFilter`,
   and the `StepwiseChange` / `MonotonicRun` / `WindowedChangeDelta` /
   `CumulativeChange` breakers).

## Out of scope

Non-deployable support code (`justfile`, `scripts/`), Stellar CLI invocations,
Reflector's and RedStone's own Stellar SEP-40 contracts, Pyth's Lazer verifier
contract and the vendored `pyth-lazer-stellar-sdk` 0.4.0 payload parser, off-chain
keepers / refresh bots, and monitoring infrastructure.

## Threat-model assumptions

- The Stellar network and Soroban host are trusted; host-level exploits are out
  of scope.
- The governance owner key is a secure multisig/process outside this boundary.
- Reflector and RedStone SEP-40 contracts report correct prices and timestamps.
- Pyth's Lazer verifier accepts only payloads signed by Pyth's trusted signer set;
  a compromised signer is a Pyth-side failure, bounded on our side by quorum.
- Ledger timestamps are accurate within Soroban's resolution; extreme clock skew
  is out of model. A source's own reported timestamp is trusted once the
  freshness filter admits it, and becomes the history's monotonic head.
- A batched `refresh_many` is all-or-nothing on a trap: an archived entry in a
  third-party source reverts the sweep for every asset, not just that one.
  Keepers size batches accordingly; the alternative, reporting a trapped
  refresh as a status, would need a new `RefreshStatus` variant in the audited
  shared crate.
- The batcher is never granted a role or a token balance. It forwards calls to
  caller-supplied addresses, so it is the direct caller for them; a target that
  checks `require_auth` on the batcher address would be satisfied. Nothing is
  exposed while the batcher holds nothing, and it is deployed ownerless with no
  `upgrade`, which the release gate enforces against its Wasm.
- One `max_age_secs` filters every source, so it has to clear the slowest:
  RedStone fires on deviation or a 12h heartbeat and Reflector buckets
  timestamps to its 300s resolution. A window wide enough for RedStone is
  therefore wide enough to serve a hours-old aggregate. The breaker set, not
  the freshness filter, is the control on that exposure, which is why pegged
  assets carry tight `StepwiseChange` bounds.
- An off-chain keeper calls `extend_ttl` at least weekly; eviction from missed
  TTL calls is an operational risk, not a contract bug. Every contract's
  `extend_ttl` renews both its instance and its persistent entries, so a Lazer
  feed that stops being pushed still survives on keeper maintenance alone. Only
  a keeper that stops entirely archives an entry, and an archived entry makes
  that asset's refresh need a restore rather than degrading to
  `SourceUnavailable`.
- Release and rehearsal tooling runs in a trusted workstation/CI environment,
  but treats files, CLI/RPC output, and checkpoint contents as untrusted input.

## Safety topics

**Oracle manipulation.** `set_proxy` requires 3–16 distinct oracle addresses,
`min_sources` in `[3, sources.len()]`, and both freshness bounds. `refresh`
drops stale or future-dated sources before aggregation and fails closed if its
per-asset storage is missing. Breakers require governance proposals to
add/update/remove; manual trips require the `ManualTripper` role.

**Governance and authorization.** Every runtime state-change except `refresh`
requires owner (`#[only_owner]`) authorization. Proposals are created and
executed by id after maturity; the 64-pending cap bounds vector growth.
Execution cannot precede the per-kind TTL (`effective_ttl` = max of requested
and configured minimum). `SetActionTtl` requires `ProxyConfigurationManager`
(Admin overrides). Revoking the last `Admin` is rejected (`LastAdmin`).
Ownership handoff emits `OwnershipTransferSubmitted` on submit and completes when
the new owner calls `accept_ownership`; monitoring should alert on ownership
transfer immediately.

**Storage and resources.** `extend_ttl(asset)` first requires a registered
proxy, then guards every potentially-absent asset key before extending it and
emits `TtlExtended`; arbitrary assets cannot create maintenance-event noise.
Optimized WASM budgets are runtime/governance ≤ 128 KiB and adapter/Lazer
source/batcher ≤ 32 KiB. The release gate enforces all five budgets and their
reviewed ABI policies. Each refresh handles one asset; breaker evaluation is
bounded by history length (≤ 32) and breaker count (≤ 16 per asset). All three
batcher vectors are capped at 64 before any cross-contract dispatch, so an
oversized request traps atomically with contract error 1 and produces no prefix
effects.

**Operational.** Reads fail closed — `aggregated_latest` / adapter `lastprice`
return `None` on missing config, non-`Accepted` status, a cached entry older
than the configured `max_age_secs`, or adapter parent-base drift. Any changed
`SetProxy` policy clears cache and history, and a change to the source set or
quorum also clears breakers.

`aggregated_history` and adapter `price` / `prices` return `None` while a manual
or enforced automatic breaker blocks the asset, while retaining historical
records independent of freshness otherwise. Every candidate is evaluated by
breakers before source time may advance cache/history. A non-advancing
candidate still serves the prior aggregate, and `RefreshEvaluated` records the
candidate when it differs. A failed or blocked refresh replaces the cached
result.
Every persisted breaker set is semantically valid.

**Artifact and rehearsal evidence.** A release is one clean-source,
manifest-last publication of exactly five freshly built optimized Wasms. Schema
4 binds the Git commit, supported Stellar CLI and Rust toolchain, package
versions, canonical paths, byte lengths, SHA-256 hashes, reviewed contract-spec
hashes, and per-artifact limits. Validation opens regular single-link files
without following symlinks and hashes/spec-checks the same descriptor under a
shared lock. A stale PASS is removed before any build or validation attempt.

The live rehearsal is testnet-only. It snapshots the validated release under
the same lock, fingerprints scripts, artifacts, environment, administrator, and
external provider code, then re-fetches every already-deployed rehearsal
contract and refuses drift on resume. Deterministic salts, contract IDs,
constructor arguments, and artifact hashes are re-derived before resume.
Every write follows build-only → simulate → sign → hash → persist the
prepared envelope → verify its hash → checkpoint `submitted` → send →
independent `getTransaction` polling. Signed envelopes and terminal RPC evidence
are persisted in a mode-0700, marker-protected output directory; at most one
operation may be unresolved. `--reinitialize` is the only destructive reset and
refuses to clear an unmarked non-empty directory.

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
- **Budget scope** — full Stellar CPU/memory simulation needs live RPC evidence;
  local release gates cover tests, ABI, and artifact size, not live resource
  budgets.

## Verification

```bash
just -f contract/proxy-oracle/soroban/justfile test
just -f contract/proxy-oracle/soroban/justfile test-integration
just -f contract/proxy-oracle/soroban/justfile test-scripts
just -f contract/proxy-oracle/soroban/justfile size-check
# From a clean tracked tree; builds and validates one exact five-Wasm release:
just -f contract/proxy-oracle/soroban/justfile release-gate
```
