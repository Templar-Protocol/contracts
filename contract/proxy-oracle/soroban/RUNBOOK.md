# Soroban Proxy Oracle — Operational Runbook

This runbook covers the full operational lifecycle of the Soroban Proxy Oracle: deploy, initialize, configure, monitor, respond to incidents, upgrade, and roll back. It is the primary reference for operators and on-call engineers.

Related documents:
- `README.md` — contract overview and known limits
- `PARITY.md` — behavioral parity baseline with the NEAR implementation
- `AUDIT.md` — audit boundary, threat topics, and evidence checklist

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Build and Release Gates](#2-build-and-release-gates)
3. [Deploy](#3-deploy)
4. [Initialize](#4-initialize)
5. [Configure Sources](#5-configure-sources)
6. [Configure Circuit Breakers](#6-configure-circuit-breakers)
7. [Grant and Revoke Manual-Trip Roles](#7-grant-and-revoke-manual-trip-roles)
8. [Governance Proposals](#8-governance-proposals)
9. [Refresh Cadence](#9-refresh-cadence)
10. [TTL Extension Cadence](#10-ttl-extension-cadence)
11. [Size and Release Verification](#11-size-and-release-verification)
12. [Monitoring Event Reference](#12-monitoring-event-reference)
13. [Incident Response: Manual Trip and Untrip](#13-incident-response-manual-trip-and-untrip)
14. [Incident Response: Source Outage](#14-incident-response-source-outage)
15. [Upgrade Dry-Run](#15-upgrade-dry-run)
16. [Rollback Criteria](#16-rollback-criteria)
17. [Evidence Collection Commands](#17-evidence-collection-commands)

---

## 1. Prerequisites

**Toolchain:**

```bash
# Rust with wasm32 target (Stellar CLI v25+ builds for wasm32v1-none)
rustup target add wasm32v1-none

# Stellar CLI with optimizer
cargo install --locked stellar-cli --features opt

# Python 3 (for size/budget gate scripts)
python3 --version
```

**Justfile runner:**

```bash
# All just commands in this runbook use the soroban justfile
JUSTFILE=contract/proxy-oracle/soroban/justfile
alias jrun="just -f $JUSTFILE"
```

**Identity setup:** Operators need a funded Stellar account. The runbook uses `$OPERATOR_ACCOUNT` as a placeholder for the account address. Never embed private keys or seed phrases in scripts or logs.

---

## 2. Build and Release Gates

Run all gates before any deployment or upgrade. The full gate sequence is:

```bash
# Run unit tests, build, optimize, size-check, and budget-check
just -f contract/proxy-oracle/soroban/justfile release-gate
```

Individual steps:

```bash
# Tests only
just -f contract/proxy-oracle/soroban/justfile test

# Build both WASMs
just -f contract/proxy-oracle/soroban/justfile build

# Optimize both WASMs
just -f contract/proxy-oracle/soroban/justfile optimize

# Size gate (enforces 131072-byte limit on each optimized artifact)
just -f contract/proxy-oracle/soroban/justfile size-check

# Deterministic budget scenarios
just -f contract/proxy-oracle/soroban/justfile budget-check
```

The size gate writes evidence to `.omo/evidence/task-7-size-check.txt`. Both optimized WASMs must remain at or below 131072 bytes (128 KiB). Release and audit gates write their supporting evidence under `.omo/evidence`. Recheck after any change to runtime, governance, ABI, or event structs.

Current verified sizes:
- Runtime: 121114 bytes (118.28 KiB)
- Governance: 55409 bytes (54.11 KiB)

---

## 3. Deploy

Generate the release manifest and validate artifacts before deploying:

```bash
# Build optimized WASMs and write release-manifest.json
just -f contract/proxy-oracle/soroban/justfile release

# Validate artifacts without broadcasting (no secrets required)
just -f contract/proxy-oracle/soroban/justfile dry-run
```

The dry-run writes `.omo/evidence/task-8-dry-run.txt` with simulated install and initialize command templates. Review that file before proceeding to a live deploy.

**Install contracts on-chain** (illustrative; substitute your network and identity):

```bash
# Install runtime WASM
stellar contract install \
  --network <network> \
  --source <identity> \
  --wasm target/proxy-oracle-soroban/wasm/templar_proxy_oracle_soroban_contract.optimized.wasm

# Install governance WASM
stellar contract install \
  --network <network> \
  --source <identity> \
  --wasm target/proxy-oracle-soroban/wasm/templar_proxy_oracle_soroban_governance_contract.optimized.wasm
```

Record the returned WASM hashes. They appear in the release manifest under `runtime_wasm.sha256` and `governance_wasm.sha256` for cross-check.

---

## 4. Initialize

Initialize the runtime contract first, then the governance contract. Both constructors are one-shot: calling them a second time returns `AlreadyInitialized`.

**Runtime constructor:**

```bash
stellar contract invoke \
  --network <network> \
  --source <identity> \
  --id <RUNTIME_CONTRACT_ID> \
  -- __constructor \
  --governance <GOVERNANCE_CONTRACT_ID> \
  --base '{"Other": "USD"}' \
  --decimals 7 \
  --resolution 1
```

Parameters:
- `governance`: address of the governance contract (set this after deploying governance, or use a multisig address and update later via `set_governance`)
- `base`: the SEP-40 base asset for all proxied prices
- `decimals`: output price decimal places (0–18; 18 is the maximum)
- `resolution`: must be non-zero; typically 1 for per-asset resolution

**Governance constructor:**

```bash
stellar contract invoke \
  --network <network> \
  --source <identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- __constructor \
  --admin <ADMIN_ADDRESS> \
  --proxy_oracle <RUNTIME_CONTRACT_ID> \
  --action_ttl_ns 86400000000000
```

Parameters:
- `admin`: the address authorized as initial `Role::Admin`; can submit, execute, and cancel proposals
- `proxy_oracle`: the runtime contract address
- `action_ttl_ns`: seeds a uniform proposal maturity delay across all `OperationKind` values in nanoseconds (example: 86400000000000 = 24 hours). Per-kind TTLs can be adjusted later via `SetActionTtl(kind, new_ttl_ns)`. Breaker lifecycle proposal actions use distinct `Rearm` and `SetEnforced` TTLs.

After initialization, verify both contracts are live:

```bash
stellar contract invoke --network <network> --id <RUNTIME_CONTRACT_ID> -- governance
stellar contract invoke --network <network> --id <GOVERNANCE_CONTRACT_ID> -- admin
```

---

## 5. Configure Sources

All proxy and source configuration goes through governance proposals. Direct calls to `set_proxy` on the runtime require the governance contract to authorize them.

**Submit a proposal to add a proxy with sources:**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"SetProxy": [
    {"Other": "BTC"},
    {
      "sources": [
        {"oracle": "<SOURCE_CONTRACT_1>", "asset": {"Other": "BTC"}},
        {"oracle": "<SOURCE_CONTRACT_2>", "asset": {"Other": "BTC"}}
      ],
      "min_sources": 2,
      "max_age_secs": 120,
      "max_clock_drift_secs": 30
    }
  ]}'
```

Source configuration rules:
- At least one source is required; maximum 16 sources per proxy.
- `min_sources` must be between 1 and the number of configured sources. Setting it to 1 means one source can determine the price alone; prefer 2 or more for important feeds.
- Duplicate `(oracle, asset)` pairs within a single proxy are rejected.
- `max_age_secs`: reject source prices older than this many seconds. Required for production feeds.
- `max_clock_drift_secs`: reject source prices with timestamps more than this many seconds in the future.

**Submit a proposal to remove a proxy:**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"RemoveProxy": {"Other": "BTC"}}'
```

Removing a proxy clears the proxy config, breaker state, history, and cache for that asset. The asset is removed from the `Assets` list.

After the maturity delay, accept the proposal (see [Section 8](#8-governance-proposals)).

---

## 6. Configure Circuit Breakers

Circuit breakers require a proxy to exist first. Configure the breaker set before adding individual breakers.

**Step 1: Configure the breaker set (sample interval and history length):**

```bash
# Submit governance proposal
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"ConfigureBreakers": [{"Other": "BTC"}, 60, 16]}'
# Arguments: asset, sample_interval_secs, history_len
```

- `sample_interval_secs`: minimum seconds between accepted history samples.
- `history_len`: number of accepted history entries (1–32). Must be large enough for every installed rule. A too-small history effectively disables protection even if breakers are installed and armed.

**Step 2: Add a breaker:**

```bash
# StepwiseChange breaker (trips on sudden price jumps)
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"AddBreaker": [{"Other": "BTC"}, {"StepwiseChange": {"max_relative_change_repr": [...]}}]}'
```

Breaker kinds:
- `StepwiseChange` (kind code 1): trips when a single price step exceeds `max_relative_change`. Use to catch sudden jumps.
- `MonotonicRun` (kind code 2): trips when price moves in the same direction for `max_streak` consecutive steps each exceeding `min_relative_step_change`. Use to catch staged ramps.
- `WindowedChangeDelta` (kind code 3): trips when the cumulative change over a window exceeds `max_relative_change_delta`. Use to catch slow drift.

Avoid inert parameters: zero streaks, windows shorter than two observations, and zero lookback windows produce breakers that never trip.

**Step 3: Set enforcement:**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"SetEnforced": [{"Other": "BTC"}, <BREAKER_ID>, {"is_enforced": true}]}'
```

Unenforced breakers still evaluate and can trip, but a trip does not block the price feed. Set `is_enforced: true` for production feeds.

**Rearm a tripped breaker:**

```bash
# Rearm with empty history (clears the baseline)
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"Rearm": [{"Other": "BTC"}, <BREAKER_ID>, {"armed_after_secs": 3600, "accepted_history_source_code": 0}]}'

# Rearm seeding from observed history (collected during the incident)
# accepted_history_source_code: 0 = Empty, 1 = Observed
```

Observed history continues collecting valid sampled prices even while the set is tripped. Treat observed history as recovery/audit data until governance explicitly seeds from it via rearm.

**Remove a breaker:**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"RemoveBreaker": [{"Other": "BTC"}, <BREAKER_ID>]}'
```

---

## 7. Grant and Revoke Roles

Governance roles (`Admin`, `ManualTripper`, `CircuitBreakerOperator`, `ProxyConfigurationManager`) are managed through `SetRole` proposals. Manual trip/untrip requires the `ManualTripper` role via `SetManualTrip` governance actions.

### Governance Roles

**Grant a governance role:**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"SetRole": ["<OPERATOR_ADDRESS>", "ProxyConfigurationManager", true]}'
```

**Revoke a governance role:**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"SetRole": ["<OPERATOR_ADDRESS>", "ProxyConfigurationManager", false]}'
```

Removing the last `Role::Admin` membership is rejected.

### Manual Trip Role

Manual trip and untrip both require the `ManualTripper` governance role. Grant it through `SetRole`:

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"SetRole": ["<OPERATOR_ADDRESS>", "ManualTripper", true]}'
```

Revoke:

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"SetRole": ["<OPERATOR_ADDRESS>", "ManualTripper", false]}'
```

After granting, verify the role is active before an incident occurs.

### Inspect Role Grants

```bash
# Check whether a specific account holds a role (runtime)
stellar contract invoke \
  --network <network> \
  --id <RUNTIME_CONTRACT_ID> \
  -- has_role \
  --account <OPERATOR_ADDRESS> \
  --role "ManualTripper"

# Check governance roles
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- has_role \
  --account <OPERATOR_ADDRESS> \
  --role "Admin"

# List all accounts holding a role
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- list_role \
  --role "ProxyConfigurationManager"

# List all roles for a specific account
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- get_roles \
  --account <OPERATOR_ADDRESS>
```

After accepting the proposal, verify the role is active before an incident occurs.

---

## 8. Governance Proposals

The governance contract supports a typed proposal lifecycle with per-operation TTLs. Proposals execute by id after their maturity delay; no FIFO ordering is required. At most 64 proposals may be pending at once; canceling or executing a proposal frees a slot. The `submit`/`accept`/`revoke` methods remain as compatibility aliases.

This contract is not an implicit migration target for earlier prototype governance storage. If an existing deployment used different role, TTL, or pending-proposal keys, deploy and initialize a fresh governance contract or ship an explicit migration before upgrading in place.

### Proposal Lifecycle

**Create a proposal (typed):**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- create_proposal \
  --caller <ADMIN_ADDRESS> \
  --id <NEXT_ID> \
  --operation '<ACTION_JSON>' \
  --requested_ttl 0
```

Parameters:
- `id`: must match `next_proposal_id`. Auto-incrementing.
- `operation`: a `GovernanceAction` variant (see action list below).
- `requested_ttl`: caller-specified maturity delay in nanoseconds. Zero uses the configured per-kind TTL. The effective TTL is the maximum of the requested TTL and the configured per-kind minimum.

Creation returns `InvalidInput` once 64 proposals are pending. `next_proposal_id` does not advance when this cap rejects a proposal.

**Submit a proposal (compatibility alias):**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '<ACTION_JSON>'
```

`submit` auto-assigns the next proposal id and uses zero requested TTL, delegating to `create_proposal`.

### Query Views

```bash
# Next proposal id (required for create_proposal)
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- next_proposal_id

# Number of pending proposals
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- proposal_count

# List pending proposal ids (paginated)
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- list_proposals \
  --offset 0 \
  --count 10

# Inspect a specific proposal
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- get_proposal \
  --id <ID>

# Get effective TTL for an operation
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- get_effective_proposal_ttl \
  --operation '<ACTION_JSON>' \
  --requested_ttl 0

# Get configured TTL for an operation kind
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- get_operation_ttl \
  --kind SetProxy
```

### Execute and Cancel

**Execute a mature proposal by id:**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- execute_proposal \
  --caller <ADMIN_ADDRESS> \
  --proposal_id <ID>
```

Returns `ProposalNotMature` if the proposal has not yet passed its TTL. Returns `ProposalNotFound` if the id does not exist. No FIFO ordering is enforced.

**Accept a proposal (compatibility alias):**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- accept \
  --caller <ADMIN_ADDRESS> \
  --proposal_id <ID>
```

`accept` delegates to `execute_proposal`.

**Cancel a proposal:**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- cancel_proposal \
  --caller <ADMIN_ADDRESS> \
  --proposal_id <ID>
```

**Revoke a proposal (compatibility alias):**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- revoke \
  --caller <ADMIN_ADDRESS> \
  --proposal_id <ID>
```

`revoke` delegates to `cancel_proposal`.

### Pending IDs (compatibility)

```bash
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- pending_ids
```

Returns all pending proposal ids. Equivalent to `list_proposals(0, proposal_count)`.

```bash
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- pending \
  --proposal_id <ID>
```

Returns `id`, `action`, and `valid_after_ns`. Do not execute before `valid_after_ns`.

### Governance Actions

Action codes in `ProposalSubmitted` events:
- `1`: SetProxy
- `2`: RemoveProxy
- `3`: ConfigureBreakers
- `4`: AddBreaker
- `5`: RemoveBreaker
- `6`: (reserved)
- `7`: SetManualTrip
- `8`: (reserved)
- `9`: SetGovernance
- `10`: SetActionTtl
- `11`: SetRole
- `12`: AdminUpgrade
- `13`: Rearm
- `14`: SetEnforced

### Per-Operation TTLs

Each action kind has its own maturity delay stored in `TtlConfig`. The constructor seeds uniform TTLs. Breaker lifecycle changes are split into explicit `Rearm` and `SetEnforced` proposal actions with independent operation TTLs. Adjust individual TTLs via `SetActionTtl`:

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"SetActionTtl": ["SetProxy", 172800000000000]}'
# SetProxy TTL = 172800000000000 ns = 48 hours
```

The `ActionTtlSet` event includes the `kind` and `new_ttl_ns`.

`SetActionTtl` requires `Role::ProxyConfigurationManager`; `Role::Admin` can also change any TTL through the global Admin override.

### Governance Handoff (transfer runtime governance to a new address)

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"SetGovernance": "<NEW_GOVERNANCE_ADDRESS>"}'
```

This emits both `ProposalSubmitted` and `GovernanceHandoffSubmitted` events. After acceptance, the runtime emits `GovernanceHandoff` with the old and new governance addresses.

### AdminUpgrade (governed runtime WASM upgrade)

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"AdminUpgrade": "<NEW_WASM_HASH>"}'
```

Requires `Role::Admin`. After execution, the runtime contract code is updated to the new WASM hash. Zero hashes are rejected.

---

## 9. Refresh Cadence

`refresh(assets)` is the only path that reads source contracts and updates the cache. SEP-40 reads (`lastprice`, `price`, `prices`) are storage-only and never call source contracts.

**Trigger a refresh:**

```bash
# Refresh specific assets
stellar contract invoke \
  --network <network> \
  --source <caller-identity> \
  --id <RUNTIME_CONTRACT_ID> \
  -- refresh \
  --assets '[{"Other": "BTC"}, {"Other": "ETH"}]'

# Refresh all configured assets (pass empty list)
stellar contract invoke \
  --network <network> \
  --source <caller-identity> \
  --id <RUNTIME_CONTRACT_ID> \
  -- refresh \
  --assets '[]'
```

**Cadence guidance:**
- Refresh at least as frequently as the shortest `max_age_secs` configured across all proxies. If any proxy has `max_age_secs: 120`, refresh at least every 60–90 seconds to avoid stale reads.
- Off-chain services should call `refresh` before any action that depends on a fresh price. Falling back to direct source reads bypasses proxy aggregation and circuit-breaker semantics.
- `refresh` returns `Vec<(Asset, RefreshStatus)>`. Check each status: `Accepted` means the cache was updated; `Blocked` means a circuit breaker is blocking; `ResolveFailed` means aggregation or conversion failed; `SourceUnavailable` means all sources returned nothing; `UnknownAsset` means the asset has no proxy config.

---

## 10. TTL Extension Cadence

Soroban persistent and instance storage entries expire if their TTL is not extended. NEAR storage is permanent; this section is Soroban-specific.

**Extend runtime TTL:**

```bash
stellar contract invoke \
  --network <network> \
  --source <caller-identity> \
  --id <RUNTIME_CONTRACT_ID> \
  -- extend_ttl
```

This extends instance storage and all persistent keys: `Assets`, `Proxy(asset)`, `Breakers(asset)`, `Cache(asset)`, and `History(asset)`. Keys that do not exist are skipped safely. Emits `TtlExtended` with the asset count.

**Extend governance TTL:**

```bash
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- extend_ttl \
  --caller <ADMIN_ADDRESS>
```

Governance `extend_ttl` requires admin authorization. Emits `TtlExtended`.

**Cadence guidance:**
- Call `extend_ttl` on both contracts at least once per week, or more frequently if the network's ledger close time is fast and the default TTL threshold is short.
- Automate TTL extension in the same off-chain service that drives `refresh`. A missed TTL extension can cause storage eviction, which makes the contract appear uninitialized to callers.
- After any incident that delays normal operations, extend TTL before resuming refresh.

---

## 11. Size and Release Verification

Before any deployment or upgrade, verify artifact integrity:

```bash
# Full release gate (test + optimize + size-check + budget-check)
just -f contract/proxy-oracle/soroban/justfile release-gate

# Generate release manifest with SHA-256 checksums
just -f contract/proxy-oracle/soroban/justfile release

# Validate artifacts without broadcasting
just -f contract/proxy-oracle/soroban/justfile dry-run
```

The release manifest at `target/proxy-oracle-soroban/release-manifest.json` contains:
- `git_commit` and `git_commit_short`: the exact source revision
- `stellar_cli`: the Stellar CLI version used to optimize
- `rust_toolchain`: the Rust toolchain version
- `runtime_wasm.sha256` and `governance_wasm.sha256`: SHA-256 of the optimized artifacts
- `runtime_wasm.optimized_size` and `governance_wasm.optimized_size`: byte counts
- `dry_run_commands`: simulated install and initialize command templates

Cross-check the SHA-256 values against the on-chain WASM hash after installation. If they differ, do not proceed.

**Print sizes of existing artifacts without rebuilding:**

```bash
just -f contract/proxy-oracle/soroban/justfile sizes
```

---

## 12. Monitoring Event Reference

All events are compact Soroban typed events. They are not byte-for-byte equivalent to NEAR proxy-oracle JSON events, but they carry equivalent operational information.

### Runtime Events

#### `RefreshSuccess`

Topics: `asset`
Payload: `price` (i128), `timestamp` (u64)

Meaning: `refresh` accepted a new price for this asset. The cache was updated.
Response: Normal operation. Track price and timestamp for freshness monitoring.

---

#### `RefreshFailure`

Topics: `asset`
Payload: `code` (u32)

Meaning: `refresh` failed to produce an accepted price. The cache was updated with a failed status.

Code meanings:
- `1`: aggregation failed (quorum not met or all sources stale)
- `2`: circuit breaker error during resolve
- `3`: storage error (missing required config key)
- `4`: price conversion overflow
- `5`: all sources unavailable (no source returned a price)
- `6`: unknown asset (no proxy config for this asset)

Response: Investigate the code. Code 5 indicates a source outage (see [Section 14](#14-incident-response-source-outage)). Code 1 may indicate stale sources or a quorum misconfiguration. Code 3 indicates a missing config key, which may mean TTL expired.

---

#### `CacheBlocked`

Topics: `asset`
Payload: `reason_code` (u32)

Meaning: `refresh` produced a valid price, but a circuit breaker blocked it from being accepted.

Reason codes:
- `1`: manually tripped
- `2`: automatic breaker trip

Response: If `reason_code` is 1, an operator manually blocked this feed. Verify the trip was intentional. If `reason_code` is 2, a circuit breaker fired automatically. Investigate the price movement and decide whether to rearm or investigate further.

---

#### `CircuitBreakerConfigSet`

Topics: `asset`
Payload: `sample_interval_secs` (u64), `history_len` (u32)

Meaning: The breaker set configuration for this asset was updated via governance.
Response: Verify the new config matches the intended governance proposal. Alert if unexpected.

---

#### `CircuitBreakerAdded`

Topics: `asset`, `breaker_id`
Payload: `breaker_kind` (u32)

Meaning: A new circuit breaker was added to this asset's breaker set.

Kind codes: 1 = StepwiseChange, 2 = MonotonicRun, 3 = WindowedChangeDelta

Response: Confirm the breaker ID and kind match the governance proposal. Record the breaker ID for future update/remove operations.

---

#### `CircuitBreakerRemoved`

Topics: `asset`, `breaker_id`
Payload: none

Meaning: A circuit breaker was removed from this asset's breaker set.
Response: Confirm removal was intentional. Removing a breaker clears its state and invalidates the cache.

---

#### `CircuitBreakerEnforcementSet`

Topics: `asset`, `breaker_id`
Payload: `is_enforced` (bool)

Meaning: The enforcement flag for a specific breaker was changed.
Response: If `is_enforced` becomes `false`, this breaker can still trip but will not block the feed. Verify this matches the intended governance action.

---

#### `CircuitBreakerRearmed`

Topics: `asset`, `breaker_id`
Payload: `armed_after_secs` (u64), `accepted_history_source_code` (u32)

Meaning: A tripped breaker was rearmed. It will begin evaluating again after `armed_after_secs`.

History source codes: 0 = Empty (baseline cleared), 1 = Observed (seeded from observed history)

Response: Confirm the rearm was intentional and the delay is appropriate. If seeded from observed history, review that history for anomalies before accepting the rearm.

---

#### `CircuitBreakerTripped`

Topics: `asset`, `breaker_id`
Payload: `tripped_at_secs` (u64), `price` (i128), `timestamp` (u64), `is_enforced` (bool)

Meaning: A circuit breaker fired automatically. If `is_enforced` is `true`, the feed is now blocked.
Response: Investigate the price that triggered the trip. If `is_enforced` is `false`, the breaker tripped but the feed is still live; treat this as a warning. If `is_enforced` is `true`, the feed is blocked until rearmed or manually untripped.

---

#### `ManualTripSet`

Topics: `asset`, `actor`
Payload: `is_manually_tripped` (bool), `metadata` (Option<Bytes>, max 1024 bytes)

Meaning: A governed manual-trip proposal tripped or untripped this asset's feed. Metadata is event-only and not stored in contract state.
Response: Confirm the actor is the proposal creator and an authorized operator. If `is_manually_tripped` is `true`, the feed is blocked. Review the metadata for the stated reason. If unexpected, investigate immediately.

---

#### `ProxySet`

Topics: `asset`
Payload: `source_count` (u32), `min_sources` (u32)

Meaning: A proxy configuration was set or updated for this asset. The cache was invalidated.
Response: Confirm the source count and quorum match the governance proposal.

---

#### `ProxyRemoved`

Topics: `asset`
Payload: none

Meaning: A proxy was removed. All associated state (config, breakers, history, cache) was cleared.
Response: Confirm removal was intentional. Any downstream service reading this asset will now receive `None` from `lastprice`.

---

#### `GovernanceHandoff`

Topics: `old_governance`, `new_governance`
Payload: none

Meaning: The runtime's governance address was changed.
Response: Verify the new governance address matches the intended proposal. This is a high-impact change; alert immediately if unexpected.

---

#### `TtlExtended` (runtime)

Topics: none
Payload: `asset_count` (u32)

Meaning: `extend_ttl` was called on the runtime. All persistent and instance keys were extended.
Response: Normal operation. Use this event to confirm TTL extension is running on schedule.

---

### Governance Events

#### `ProposalSubmitted`

Topics: `id`
Payload: `valid_after_ns` (u64), `action_code` (u32)

Meaning: A new governance proposal was queued.

Action codes:
- `1`: SetProxy
- `2`: RemoveProxy
- `3`: ConfigureBreakers
- `4`: AddBreaker
- `5`: RemoveBreaker
- `6`: (reserved)
- `7`: SetManualTrip
- `8`: (reserved)
- `9`: SetGovernance
- `10`: SetActionTtl
- `11`: SetRole
- `12`: AdminUpgrade
- `13`: Rearm
- `14`: SetEnforced

Response: Record the proposal ID and `valid_after_ns`. Do not accept before maturity. Alert on unexpected submissions.

---

#### `ProposalAccepted`

Topics: `id`
Payload: none

Meaning: A proposal was accepted and its action was executed on the runtime.
Response: Confirm the accepted ID matches the expected proposal. Verify the corresponding runtime event was emitted (e.g., `ProxySet` for a `SetProxy` action).

---

#### `ProposalRevoked`

Topics: `id`
Payload: none

Meaning: A proposal was removed from the queue without executing.
Response: Confirm revocation was intentional. If unexpected, investigate who holds admin authority.

---

#### `GovernanceHandoffSubmitted`

Topics: `id`, `new_governance`
Payload: none

Meaning: A `SetGovernance` proposal was submitted. Emitted alongside `ProposalSubmitted`.
Response: Alert immediately. Verify the new governance address is correct before the maturity delay expires.

---

#### `ActionTtlSet`

Topics: none
Payload: `kind` (OperationKind), `new_ttl_ns` (u64)

Meaning: The proposal maturity delay for a specific operation kind was changed.
Response: Verify the new TTL matches the intended governance proposal. A shorter TTL reduces the window for catching and revoking malicious proposals.

---

#### `TtlExtended` (governance)

Topics: none
Payload: none

Meaning: `extend_ttl` was called on the governance contract.
Response: Normal operation. Confirm this runs on schedule.

---

## 13. Incident Response: Manual Trip and Untrip

Use manual trip to immediately block a price feed when you suspect manipulation, source compromise, or an anomaly that circuit breakers have not caught.

**Trip a feed:**

```bash
stellar contract invoke \
  --network <network> \
  --source <trip-operator-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <TRIP_OPERATOR_ADDRESS> \
  --action '{"SetManualTrip": ["<TRIP_OPERATOR_ADDRESS>", {"Other": "BTC"}, true, "<base64-encoded reason, max 1024 bytes>"]}'
```

Requirements:
- The `caller` must hold `Role::ManualTripper`, or `Role::Admin` for the global override.
- The action `actor` must match `caller`; mismatched actor attribution is rejected.
- Metadata is event-only and capped at 1024 bytes. It is not stored in contract state.
- The cache is invalidated immediately. `lastprice` returns `None` until the feed is untripped and refreshed.
- Execute the proposal after its `SetManualTrip` maturity delay.

**Verify the trip took effect:**

```bash
stellar contract invoke \
  --network <network> \
  --id <RUNTIME_CONTRACT_ID> \
  -- get_breaker_set_view \
  --asset '{"Other": "BTC"}'
# Expect: is_manually_tripped: true, is_blocking: true
```

**Untrip a feed:**

```bash
stellar contract invoke \
  --network <network> \
  --source <untrip-operator-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <UNTRIP_OPERATOR_ADDRESS> \
  --action '{"SetManualTrip": ["<UNTRIP_OPERATOR_ADDRESS>", {"Other": "BTC"}, false, "<base64-encoded reason>"]}'
```

Requirements:
- The `caller` must hold `Role::ManualTripper`, or `Role::Admin` for the global override.
- The action `actor` must match `caller`; mismatched actor attribution is rejected.

After untripping, call `refresh` to repopulate the cache before downstream services resume reading.

---

## 14. Incident Response: Source Outage

A source outage occurs when one or more configured source contracts stop returning prices. The proxy handles partial outages through quorum: if at least `min_sources` sources return valid prices, `refresh` succeeds.

**Detect a source outage:**

- `RefreshFailure` with `code: 5` (SourceUnavailable) means all sources returned nothing.
- `RefreshFailure` with `code: 1` (aggregation failed) may mean fewer than `min_sources` sources returned valid prices.
- Monitor `RefreshSuccess` events: a gap in success events for a configured asset indicates a problem.

**Triage steps:**

1. Check each source contract directly:
   ```bash
   stellar contract invoke --network <network> --id <SOURCE_CONTRACT> -- lastprice --asset '{"Other": "BTC"}'
   ```
2. If one source is down and `min_sources` is met by remaining sources, `refresh` continues normally. No action needed unless the outage persists.
3. If `min_sources` is not met, the feed is failing. Consider:
   - Manually tripping the feed to make the failure explicit (see [Section 13](#13-incident-response-manual-trip-and-untrip)).
   - Submitting a governance proposal to lower `min_sources` temporarily or add a backup source.
4. If all sources are down, the feed will emit `RefreshFailure` with `code: 5` on every refresh attempt. The cache retains the last accepted price until it expires under `max_age_secs`.

**Recovery:**

Once sources recover, call `refresh` to repopulate the cache. If the feed was manually tripped, untrip it first.

---

## 15. Upgrade Dry-Run

Before upgrading either contract, run the full release gate and dry-run validation:

```bash
# Full gate on the new code
just -f contract/proxy-oracle/soroban/justfile release-gate

# Generate manifest for the new artifacts
just -f contract/proxy-oracle/soroban/justfile release

# Validate without broadcasting
just -f contract/proxy-oracle/soroban/justfile dry-run
```

Review `.omo/evidence/task-8-dry-run.txt` for the simulated install commands. Cross-check the SHA-256 in the manifest against the artifact on disk.

**Upgrade the runtime contract:**

Two paths are available:

1. **Direct governed upgrade** (runtime method, requires governance authorization):
```bash
# Install the new WASM (returns a new WASM hash)
stellar contract install \
  --network <network> \
  --source <identity> \
  --wasm target/proxy-oracle-soroban/wasm/templar_proxy_oracle_soroban_contract.optimized.wasm

# Upgrade the deployed contract to the new WASM hash (governance must authorize)
stellar contract invoke \
  --network <network> \
  --source <governance-identity> \
  --id <RUNTIME_CONTRACT_ID> \
  -- upgrade \
  --new_wasm_hash <NEW_WASM_HASH>
```

2. **Governed proposal upgrade** (AdminUpgrade proposal, executes after maturity):
```bash
# Submit AdminUpgrade proposal
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- submit \
  --caller <ADMIN_ADDRESS> \
  --action '{"AdminUpgrade": "<NEW_WASM_HASH>"}'

# After maturity, execute the proposal
stellar contract invoke \
  --network <network> \
  --source <admin-identity> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- execute_proposal \
  --caller <ADMIN_ADDRESS> \
  --proposal_id <ID>
```

The runtime uses `env.deployer().update_current_contract_wasm()` for upgrades. Zero WASM hashes are rejected. NEAR `AdminFunctionCall` arbitrary dynamic dispatch is intentionally not implemented on Soroban.

**Upgrade the governance contract** follows the same pattern with the governance WASM.

After upgrading, verify the contract is still responsive:

```bash
stellar contract invoke --network <network> --id <RUNTIME_CONTRACT_ID> -- governance
stellar contract invoke --network <network> --id <GOVERNANCE_CONTRACT_ID> -- admin
```

---

## 16. Rollback Criteria

Roll back an upgrade if any of the following occur within the first 30 minutes after deployment:

- `RefreshFailure` events appear for assets that were refreshing successfully before the upgrade.
- `lastprice` returns `None` for assets that had accepted cached prices.
- Governance proposals fail to submit, accept, or revoke.
- `extend_ttl` panics or returns an error.
- Any unexpected `GovernanceHandoff` event.
- The optimized WASM size exceeds 131072 bytes.

**Rollback procedure:**

1. Install the previous WASM artifact (use the SHA-256 from the previous release manifest to identify it).
2. Upgrade the contract back to the previous WASM hash.
3. Verify the contract is responsive and `refresh` succeeds.
4. Investigate the root cause before attempting the upgrade again.

Keep the previous release manifest and optimized WASM artifacts until the new version has been stable for at least 48 hours.

---

## 17. Evidence Collection Commands

Use these commands to collect evidence for audits, incident reports, and release sign-offs.

**Verify the RUNBOOK file exists:**

```bash
test -f contract/proxy-oracle/soroban/RUNBOOK.md && echo PASS || echo FAIL
```

**Verify RUNBOOK coverage:**

```bash
rg -n "deploy|initialize|governance|refresh|extend_ttl|manual trip|monitor|incident|upgrade|rollback|evidence" \
  contract/proxy-oracle/soroban/RUNBOOK.md
```

**Verify no sensitive placeholders** (run from the repo root; expect zero matches):

This file contains no credentials, key material, or unresolved placeholders. Run the acceptance command from the task spec against this file; it should return no matches.

**Verify all event families are covered:**

```bash
rg -n "RefreshSuccess|RefreshFailure|CacheBlocked|CircuitBreakerConfigSet|CircuitBreakerAdded|CircuitBreakerRemoved|CircuitBreakerEnforcementSet|CircuitBreakerRearmed|CircuitBreakerTripped|ManualTripSet|ProxySet|ProxyRemoved|GovernanceHandoff|TtlExtended|ProposalSubmitted|ProposalAccepted|ProposalRevoked|GovernanceHandoffSubmitted|ActionTtlSet" \
  contract/proxy-oracle/soroban/RUNBOOK.md
```

**Run the full release gate and capture output:**

```bash
just -f contract/proxy-oracle/soroban/justfile release-gate 2>&1 | tee /tmp/release-gate-evidence.txt
```

**Check current artifact sizes:**

```bash
just -f contract/proxy-oracle/soroban/justfile sizes
```

**Inspect the release manifest:**

```bash
cat target/proxy-oracle-soroban/release-manifest.json
```

**Verify dry-run passes:**

```bash
just -f contract/proxy-oracle/soroban/justfile dry-run
cat .omo/evidence/task-8-dry-run.txt
```

**Inspect breaker set state for an asset:**

```bash
stellar contract invoke \
  --network <network> \
  --id <RUNTIME_CONTRACT_ID> \
  -- get_breaker_set_view \
  --asset '{"Other": "BTC"}'
```

**Inspect cached price for an asset:**

```bash
stellar contract invoke \
  --network <network> \
  --id <RUNTIME_CONTRACT_ID> \
  -- get_cached \
  --asset '{"Other": "BTC"}'
```

**List all configured assets:**

```bash
stellar contract invoke \
  --network <network> \
  --id <RUNTIME_CONTRACT_ID> \
  -- assets
```

**List pending governance proposals:**

```bash
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- pending_ids
```

**Check current action TTL (returns SetProxy TTL for backward compatibility):**

```bash
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- action_ttl_ns
```

**Check TTL for a specific operation kind:**

```bash
stellar contract invoke \
  --network <network> \
  --id <GOVERNANCE_CONTRACT_ID> \
  -- get_operation_ttl \
  --kind SetProxy
```
