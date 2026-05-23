# Proxy Oracle Audit Boundary

This document defines the audit boundary, safety invariants, evidence index, threat model assumptions, known limitations, and non-goals for the Soroban Proxy Oracle implementation.

Related documents:
- `README.md` — contract overview and known limits
- `PARITY.md` — behavioral parity baseline with the NEAR implementation
- `RUNBOOK.md` — operational runbook: deploy, configure, monitor, incident response, upgrade

---

## Audit Boundary

### In-Scope Components

1. **Runtime Contract**: `contract/proxy-oracle/soroban/contract/src/lib.rs`
   - SEP-40 interface implementation (`base`, `assets`, `decimals`, `resolution`, `price`, `prices`, `lastprice`).
   - Cache management, cache invalidation, and fail-closed read semantics.
   - Source IO and kernel integration via `refresh(assets)`.
   - Storage TTL management (`extend_ttl`).
   - Manual trip / untrip authority split (`OfflineManualTrip`, `OfflineManualUntrip`).
   - Compact Soroban typed event emission covering all state-change paths.

2. **Governance Contract**: `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`
   - Proposal submission, maturity enforcement, and FIFO execution.
   - Timelock enforcement (`valid_after_ns` derived from `action_ttl_ns`).
   - Cross-contract authorization via `Address::require_auth()`.
   - `revoke` path and proposal ordering guarantees.

3. **Shared DTOs and Errors**: `contract/proxy-oracle/soroban/common/src/lib.rs`
   - Data structures, error types, and shared event schemas.
   - `GovernanceAction` enum variants covering all mutation paths.

4. **Kernel**: `templar-proxy-oracle-kernel`
   - `MedianLow` aggregation logic.
   - `FreshnessFilter` enforcement.
   - Circuit breaker evaluation (`StepwiseChange`, `MonotonicRun`, `WindowedChangeDelta`).

### Out-of-Scope Components

The following support operations but are not deployable contract code:

- `contract/proxy-oracle/soroban/justfile` — build, test, and release gate orchestration.
- `contract/proxy-oracle/soroban/scripts/` — Python helpers for size/budget checks and release manifest generation.
- Stellar CLI invocations for `contract install`, `contract deploy`, and `contract invoke`.
- RedStone Stellar SEP-40 wrapper contracts (owned and audited by RedStone).
- Off-chain keepers and refresh bots.
- Monitoring infrastructure and alerting pipelines.

---

## Threat Model Assumptions

- The Stellar network and Soroban host are trusted execution environments; host-level exploits are out of scope.
- The governance admin key is controlled by a secure multisig or governance process outside this audit boundary.
- RedStone SEP-40 wrapper contracts report correct prices and timestamps; price manipulation at the RedStone adapter layer is out of scope.
- Stellar ledger timestamps are accurate within Soroban's timestamp resolution; extreme clock skew is outside this model.
- The deployer ran `just release` and `just dry-run` before deployment and verified SHA-256 values in the release manifest against on-chain WASM hashes.
- `extend_ttl` is called at least weekly by an off-chain keeper; storage eviction due to missed TTL calls is an operational risk, not a contract logic bug.
- The deployment and upgrade tooling (`justfile`, scripts) is run in a trusted environment with no hostile inputs.

---

## Threat and Safety Topics

### Oracle Manipulation

- **Quorum Bypass**: `set_proxy` rejects `min_sources == 0` and `min_sources > sources.len()`. Governance proposals cannot set a zero-quorum proxy config. Empty source lists are rejected with `TooManySources`. Evidence: `.omo/evidence/task-3-duplicate-source.txt`.
- **Stale Price Injection**: `FreshnessFilter` is enforced in `refresh_one` via `source_kernel_price`. Sources whose timestamp predates `max_age_secs` are rejected before aggregation. Evidence: parity matrix row "Stale Source" in `PARITY.md`.
- **Breaker Evasion**: Circuit breakers require governance proposals to add, update, or remove. Emergency manual operations require the appropriate split offline role with `actor.require_auth()`. Removing a breaker via governance also invalidates the cache. Evidence: `.omo/evidence/task-6-breaker-trip-parity.txt`.
- **Missing Config Silent Failure**: `refresh_one` fails closed on missing storage keys, returning `RESOLVE_FAILED_STORAGE_CODE` rather than accepting a price. `lastprice` returns `None` on missing proxy config. Evidence: `.omo/evidence/task-5-missing-decimals.txt`, `.omo/evidence/task-5-ttl-coverage.txt`.

### Governance and Authorization

- **Unauthorized Mutation**: All runtime state-changing methods except `refresh` and `set_manual_trip` require governance authorization (`governance.require_auth()`). Manual trip/untrip require the corresponding split offline role. Evidence: `.omo/evidence/task-4-tripper-cannot-untrip.txt`, `.omo/evidence/task-6-manual-trip-parity.txt`.
- **Proposal Replay**: Governance enforces ordered execution via `NextProposalId`. Accepted proposals cannot be re-accepted. Evidence: `.omo/evidence/task-6-governance-ordering-parity.txt`.
- **Timelock Bypass**: Proposals cannot be accepted before `valid_after_ns`. The `action_ttl_ns` parameter cannot be zero; `submit` fails closed with `MissingConfig` if it is absent from storage. Evidence: task 5 fail-closed governance tests.
- **Governance Handoff Race**: `SetGovernance` emits `GovernanceHandoffSubmitted` on submit and `GovernanceHandoff` on accept. Monitoring should alert immediately on `GovernanceHandoff` events.

### Storage and Resource Limits

- **Storage Eviction**: `extend_ttl` guards all potentially-absent persistent keys with `storage.has(key)` before extending. Missing keys are skipped safely. Emits `TtlExtended` for monitoring. Evidence: `.omo/evidence/task-5-ttl-coverage.txt`.
- **WASM Size**: Optimized contract sizes must remain at or below `131072` bytes (128 KiB). Current verified sizes: runtime 115399 bytes (112.69 KiB), governance 38997 bytes (38.08 KiB). Evidence: `.omo/evidence/task-7-size-check.txt`.
- **Gas Exhaustion**: `refresh` calls are bounded by configured source count and asset list length. Asset lists are deduplicated before processing. Breaker evaluation is bounded by history length (max 32) and breaker count (max 16 per asset).

### Operational Safety

- **Fail Closed**: `lastprice` returns `None` if the proxy config is missing, the cached status is not `Accepted`, or the cached timestamp is older than `max_age_secs`. There is no default price fallback.
- **Manual Trip Semantics**: Emergency trips immediately block the feed and invalidate the cache. Metadata is event-only, capped at 1024 bytes, and not stored in breaker state. Evidence: `.omo/evidence/task-4-metadata-limit.txt`, `.omo/evidence/task-4-tripper-cannot-untrip.txt`.
- **Cache Invalidation**: Any governance mutation to proxy config, breaker config, or breaker set clears the cached price. Stale-epoch callbacks (NEAR pattern) are handled here by explicit cache removal on config change. Evidence: `.omo/evidence/task-6-refresh-and-cache-parity.txt`.

---

## Known Limitations and Non-Goals

- **Behavioral parity, not byte parity**: Soroban events are compact typed XDR events and are not byte-for-byte equivalent to NEAR proxy-oracle JSON events. Parity is verified at the semantic/outcome level. See `PARITY.md`.
- **RedStone wrapper dependency**: RedStone price data enters through RedStone's Stellar SEP-40 wrapper contracts. This proxy does not verify RedStone signatures; correctness of the RedStone adapter is a dependency, not an in-scope assertion.
- **Soroban TTL**: NEAR storage is permanent; Soroban storage is not. TTL expiry is a liveness risk. Operators must run `extend_ttl` on a regular cadence per `RUNBOOK.md` Section 10.
- **No new source aggregation framework**: The kernel shares code with the NEAR proxy oracle. No additional aggregation algorithms were introduced in this implementation.
- **No live mainnet deployment**: This document describes the contract at commit `64bf8b821cabbc94e4591ca89997c8ec00f365c7`. No claim is made about live mainnet deployment status.
- **Single `action_ttl_ns`**: Governance uses one maturity delay for all proposal kinds, matching NEAR proxy-oracle governance rather than the vault per-kind timelock model.
- **Synchronous refresh only**: All source IO occurs synchronously within a single `refresh` transaction. NEAR's async cross-contract callback pattern is not available in Soroban.
- **Budget simulation scope**: Full Stellar resource simulation (CPU/memory instruction counts) requires a live Soroban RPC endpoint and is not available locally. The `budget-check` gate runs deterministic soroban-sdk testutils scenarios as the narrowest available local harness.

---

## Evidence Checklist

All items verified as of baseline commit `64bf8b821cabbc94e4591ca89997c8ec00f365c7`.

### Parity

- [x] **parity matrix**: `PARITY.md` documents all behavioral parity rows between NEAR and Soroban.
  - Evidence: `.omo/evidence/task-1-parity-matrix.txt`
  - Verification: `test -f contract/proxy-oracle/soroban/PARITY.md`
- [x] **parity refresh and cache**: Accepted refresh, stale source, quorum failure, base mismatch, cache invalidation.
  - Evidence: `.omo/evidence/task-6-refresh-and-cache-parity.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils parity_config_update -- --nocapture`
- [x] **parity manual trip**: Split roles, event fields, metadata cap.
  - Evidence: `.omo/evidence/task-6-manual-trip-parity.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils parity_manual_trip -- --nocapture`
- [x] **parity breaker trip**: Automatic trip, observed history, rearm.
  - Evidence: `.omo/evidence/task-6-breaker-trip-parity.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils parity_breaker_trip -- --nocapture`
- [x] **parity governance ordering**: FIFO accept and revoke unblocks later proposals.
  - Evidence: `.omo/evidence/task-6-governance-ordering-parity.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-governance-contract --features testutils parity -- --nocapture`

### Events

- [x] **events golden tests**: All runtime and governance events verified by typed XDR assertions in unit tests.
  - Evidence: `.omo/evidence/task-2-refresh-event.txt`, `.omo/evidence/task-2-governance-event-failure.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils -- --nocapture`
- [x] **events monitoring coverage**: All 20 event families documented in `RUNBOOK.md` Section 12 with topics, payload, meaning, and response guidance.
  - Evidence: `.omo/evidence/task-9-monitoring-coverage.txt`
  - Verification: `rg -c "RefreshSuccess|RefreshFailure|CacheBlocked|ManualTripSet|CircuitBreakerTripped|ProposalSubmitted|ProposalAccepted|ProposalRevoked" contract/proxy-oracle/soroban/RUNBOOK.md`

### Auth

- [x] **auth governance authorization**: All configuration mutations require `governance.require_auth()`.
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils -- --nocapture`
- [x] **auth manual-trip split roles**: `OfflineManualTrip` cannot untrip; `OfflineManualUntrip` is required to clear a manual trip.
  - Evidence: `.omo/evidence/task-4-tripper-cannot-untrip.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils manual_trip_role_tripper_cannot_untrip_without_untrip_role -- --nocapture`
- [x] **auth duplicate source rejection**: `set_proxy` rejects duplicate `(oracle, asset)` pairs.
  - Evidence: `.omo/evidence/task-3-duplicate-source.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils invalid_config_duplicate_source -- --nocapture`
- [x] **auth require_auth enforcement**: `set_manual_trip` enforces `actor.require_auth()`; governance actions enforce `admin.require_auth()`.
  - Evidence: `.omo/evidence/task-6-manual-trip-parity.txt`

### Validation

- [x] **validation constructor**: `decimals > 18` and `resolution == 0` are rejected at construction time.
  - Evidence: `.omo/evidence/task-5-missing-decimals.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils invalid_config_zero_resolution -- --nocapture`
- [x] **validation min_sources**: Zero `min_sources` and `min_sources > source count` are rejected.
  - Evidence: `.omo/evidence/task-3-duplicate-source.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils invalid_config_zero_sources -- --nocapture`
- [x] **validation inert breaker params**: Zero `max_relative_change`, zero `min_relative_step_change`, and zero `max_relative_change_delta` are rejected to prevent silently inert breakers.
  - Evidence: `.omo/evidence/task-3-inert-breaker.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils inert_breaker -- --nocapture`
- [x] **validation history_len**: Zero `history_len` in `configure_breakers` is rejected.
  - Evidence: `.omo/evidence/task-3-inert-breaker.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils invalid_config_zero_history_len -- --nocapture`
- [x] **validation metadata cap**: Manual-trip metadata capped at 1024 bytes; over-limit is rejected.
  - Evidence: `.omo/evidence/task-4-metadata-limit.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils manual_trip_metadata_limit -- --nocapture`

### TTL

- [x] **TTL coverage**: All persistent and instance storage keys use `extend_ttl`; missing keys guarded with `storage.has()` to prevent host panics.
  - Evidence: `.omo/evidence/task-5-ttl-coverage.txt`
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils ttl -- --nocapture`
- [x] **TTL fail-closed on eviction**: `refresh_one` returns `RESOLVE_FAILED_STORAGE_CODE` on missing required config key; `lastprice` returns `None` on missing proxy config. No silent price defaults on TTL expiry.
  - Evidence: `.omo/evidence/task-5-ttl-coverage.txt`

### Tests

- [x] **unit tests runtime**: All runtime scenarios covered; parity, event, auth, validation, TTL, and breaker paths all tested.
  - Verification: `cargo test -p templar-proxy-oracle-soroban-contract --features testutils -- --nocapture`
- [x] **unit tests governance**: Governance lifecycle, FIFO ordering, revoke, and fail-closed paths covered.
  - Verification: `cargo test -p templar-proxy-oracle-soroban-governance-contract --features testutils -- --nocapture`
- [x] **unit tests kernel**: Shared aggregation and breaker logic covered.
  - Verification: `cargo test -p templar-proxy-oracle-kernel --features serde --lib -- --nocapture`
- [x] **full test suite**: Combined via `just -f contract/proxy-oracle/soroban/justfile test`.

### Budget

- [x] **budget deterministic scenarios**: Soroban-sdk testutils scenarios exercise `refresh`, `lastprice`, governance accept, and breaker paths. Full Stellar resource simulation requires a live Soroban RPC endpoint and is not available locally; this gate verifies scenario correctness as the narrowest available local harness.
  - Evidence: `.omo/evidence/task-7-budget-check.txt`
  - Verification: `just -f contract/proxy-oracle/soroban/justfile budget-check`

### Size

- [x] **optimized_size runtime**: 115399 bytes (112.69 KiB), limit 131072 bytes.
  - Evidence: `.omo/evidence/task-7-size-check.txt`
  - Verification: `just -f contract/proxy-oracle/soroban/justfile size-check`
- [x] **optimized_size governance**: 38997 bytes (38.08 KiB), limit 131072 bytes.
  - Evidence: `.omo/evidence/task-7-size-check.txt`
  - Verification: `just -f contract/proxy-oracle/soroban/justfile size-check`

### Release Artifacts

- [x] **release_manifest**: Release manifest written to `target/proxy-oracle-soroban/release-manifest.json` with package version, git commit, Stellar CLI version, Rust toolchain, SHA-256 checksums, optimized sizes, and dry-run command templates.
  - Evidence: `.omo/evidence/task-8-release-manifest.json`
  - Verification: `just -f contract/proxy-oracle/soroban/justfile release`
- [x] **dry-run validation**: Artifacts validated without broadcasting; integrity confirmed by SHA-256 re-check.
  - Evidence: `.omo/evidence/task-8-dry-run.txt`
  - Verification: `just -f contract/proxy-oracle/soroban/justfile dry-run`

### Runbook

- [x] **runbook exists**: `contract/proxy-oracle/soroban/RUNBOOK.md` with 17 sections covering deploy through rollback.
  - Evidence: `.omo/evidence/task-9-runbook-syntax.txt`
  - Verification: `test -f contract/proxy-oracle/soroban/RUNBOOK.md`
- [x] **runbook monitoring**: All 20 event families documented with response guidance.
  - Evidence: `.omo/evidence/task-9-monitoring-coverage.txt`
  - Verification: `rg -c "RefreshSuccess|RefreshFailure|CacheBlocked|ManualTripSet" contract/proxy-oracle/soroban/RUNBOOK.md`

### Audit-Ready Gate

- [x] **audit-ready gate**: `just -f contract/proxy-oracle/soroban/justfile audit-ready` runs all required checks and writes `.omo/evidence/soroban-proxy-oracle-audit-ready.txt`.
  - Evidence: `.omo/evidence/task-10-audit-ready.txt`
  - Verification: `just -f contract/proxy-oracle/soroban/justfile audit-ready`
- [x] **no unresolved placeholders**: No unresolved placeholder words in `AUDIT.md` or the audit-ready evidence file.
  - Evidence: `.omo/evidence/task-10-no-placeholders.txt`
  - Verification: `just -f contract/proxy-oracle/soroban/justfile audit-ready` (placeholder check is included in the gate)
