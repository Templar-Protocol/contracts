# Soroban Proxy Oracle — Operational Runbook

Deploy, configure, monitor, respond to incidents, upgrade, and roll back.
Companion docs: `README.md` (overview), `PARITY.md` (NEAR parity), `AUDIT.md`
(boundary).

## Conventions

Examples assume these are exported, and use the `inv` helper for brevity:

```bash
export NET=<network> SRC=<identity>                 # network + signing identity
export RT=<runtime_id> GOV=<governance_id> AD=<adapter_id>
JF=contract/proxy-oracle/soroban/justfile
inv() { stellar contract invoke --network "$NET" --source "$SRC" "$@"; }
```

So `inv --id "$GOV" -- next_proposal_id` runs a view. Never put key material in
scripts or logs.

## 1. Build and release gates

```bash
just -f $JF release-gate   # test + optimize + size-check + budget-check
just -f $JF release        # write target/proxy-oracle-soroban/release-manifest.json
just -f $JF dry-run        # validate artifacts (SHA-256 + size), no broadcast
```

Size budgets: runtime & governance ≤ 131072 bytes (128 KiB), adapter ≤ 32768
(32 KiB), enforced by `size-check`. The manifest records git commit, Stellar CLI
and toolchain versions, SHA-256 checksums, and optimized sizes — cross-check the
SHA-256 against the on-chain hash after install.

## 2. Deploy

Install each optimized WASM and record the returned hash:

```bash
stellar contract install --network $NET --source $SRC \
  --wasm target/proxy-oracle-soroban/wasm/<artifact>.optimized.wasm
```

for `templar_proxy_oracle_soroban_contract`, `..._governance_contract`, and
`..._sep40_adapter_contract`.

## 3. Initialize

Constructors are one-shot (`AlreadyInitialized` on re-call). Initialize the
runtime, then governance:

```bash
inv --id $RT  -- __constructor --governance $GOV --base '{"Other":"USD"}'
inv --id $GOV -- __constructor --admin <ADMIN> --proxy_oracle $RT \
    --initial_uniform_ttl_ns 86400000000000   # 24h, uniform across all OperationKinds
```

- `base` is the source-validation invariant — every source's `base()` must match
  it. Per-feed `decimals`/`resolution` are adapter-side, not here.
- Tune per-kind maturity later with `SetActionTtl`.

Hand the runtime's owner to governance via the two-step `Ownable` transfer:

```bash
inv --id $RT -- transfer_ownership --new_owner $GOV --live_until_ledger <MAX_TTL_LEDGER>
# new owner finalizes: accept_ownership directly (EOA) or an AcceptOwnership proposal (governance)
inv --id $RT -- get_owner          # verify == $GOV
inv --id $GOV -- proxy_oracle      # verify == $RT
```

Deploy one adapter per feed (`decimals ≤ 18`, `resolution ≠ 0`):

```bash
stellar contract deploy --network $NET --source $SRC --wasm-hash <ADAPTER_HASH> -- \
  --owner <OWNER> --parent_oracle $RT --asset '{"Other":"BTC"}' \
  --decimals 8 --resolution 1 --base '{"Other":"USD"}'
```

## 4. Governance proposals

Every config, role, ownership, and upgrade change is a governance proposal; the
owner (governance) authorizes the resulting runtime call. Lifecycle:

```bash
inv --id $GOV -- create_proposal --caller <ADDR> --id <NEXT> \
    --operation '<ACTION_JSON>' --requested_ttl 0
inv --id $GOV -- execute_proposal --caller <ADDR> --id <ID>   # after maturity
inv --id $GOV -- cancel_proposal  --caller <ADDR> --id <ID>   # frees a slot
```

- `id` must equal `next_proposal_id`. `requested_ttl 0` uses the configured
  per-kind minimum; the effective TTL is `max(requested, minimum)`.
- A 64-pending cap returns `InvalidInput`; `execute` before maturity returns
  `ProposalNotMature`. Execution is by id — no FIFO ordering.
- Queries: `next_proposal_id`, `active_ids`, `get_proposal --id`,
  `get_operation_ttl --kind`, `get_effective_proposal_ttl --operation --requested_ttl`.

`<ACTION_JSON>` is a `GovernanceAction` variant. Its `action_code` appears in the
`ProposalSubmitted` event; Admin overrides every role.

| code | action | required role |
|------|--------|---------------|
| 1 | `SetProxy(asset, config)` | ProxyConfigurationManager |
| 2 | `RemoveProxy(asset)` | ProxyConfigurationManager |
| 3 | `ConfigureBreakers(asset, sample_interval_secs, history_len)` | ProxyConfigurationManager |
| 4 | `AddBreaker(asset, config)` | ProxyConfigurationManager |
| 5 | `RemoveBreaker(asset, breaker_id)` | ProxyConfigurationManager |
| 6 | `RenounceOwnership(())` | Admin |
| 7 | `SetManualTrip(asset, tripped, metadata)` | ManualTripper |
| 8 | `AcceptOwnership(())` | Admin |
| 9 | `TransferOwnership(new_owner)` | Admin |
| 10 | `SetActionTtl(kind, new_ttl_ns)` | ProxyConfigurationManager |
| 11 | `SetRole(account, role, set)` | Admin |
| 12 | `Upgrade(new_wasm_hash)` | Admin |
| 13 | `Rearm(asset, breaker_id, config)` | CircuitBreakerOperator |
| 14 | `SetEnforced(asset, breaker_id, config)` | CircuitBreakerOperator |

## 5. Configure sources, breakers, and roles

Wrap each action in `create_proposal` + `execute_proposal` (examples show only
the `--action` JSON).

**Sources** — `SetProxy` / `RemoveProxy`:

```json
{"SetProxy": [{"Other":"BTC"}, {
  "sources": [{"oracle":"<SRC1>","asset":{"Other":"BTC"}},
              {"oracle":"<SRC2>","asset":{"Other":"BTC"}}],
  "min_sources": 2, "max_age_secs": 120, "max_clock_drift_secs": 30 }]}
```

1–16 sources; `min_sources ∈ [1, n]`; no duplicate `(oracle, asset)`;
`max_age_secs` required for production. `RemoveProxy` clears the proxy, breakers,
history, and cache for the asset.

**Breakers** — configure the set, add a breaker, then enforce it:

```json
{"ConfigureBreakers": [{"Other":"BTC"}, 60, 16]}                          // sample_interval_secs, history_len (1–32)
{"AddBreaker": [{"Other":"BTC"}, {"StepwiseChange": {"max_relative_change":"<hex>"}}]}
{"SetEnforced": [{"Other":"BTC"}, <BREAKER_ID>, {"is_enforced": true}]}
{"Rearm": [{"Other":"BTC"}, <BREAKER_ID>, {"armed_after_secs": 3600, "accepted_history_source_code": 0}]}
```

Kinds: `StepwiseChange` (1, sudden jumps), `MonotonicRun` (2, staged ramps),
`WindowedChangeDelta` (3, slow drift). Unenforced breakers still evaluate but a
trip does not block the feed. `history_len` must cover every installed rule —
too small silently disables protection. `accepted_history_source_code`: `0` =
Empty (clear baseline), `1` = Observed (seed from history collected while
tripped). Inert params (zero thresholds/streaks/lookback, window < 2) are
rejected.

**Roles** — `SetRole(account, role, set)` for `Admin`, `ManualTripper`,
`CircuitBreakerOperator`, `ProxyConfigurationManager`. The last `Admin` cannot be
revoked. Inspect on governance: `has_role --account --role`, `list_role --role`,
`get_roles --account`.

## 6. Refresh cadence

`refresh` is the only path that reads source contracts; all other reads are
storage-only.

```bash
inv --id $RT -- refresh --assets '[{"Other":"BTC"}]'   # or '[]' for all configured
```

Returns `Vec<(Asset, RefreshStatus)>`: `Accepted` (cache updated), `Blocked`
(breaker), `ResolveFailed` (aggregation/conversion), `SourceUnavailable` (no
source responded), `UnknownAsset`. Refresh at least as often as the shortest
`max_age_secs`, and before any action depending on a fresh price.

## 7. TTL extension

```bash
inv --id $RT  -- extend_ttl
inv --id $GOV -- extend_ttl --caller <ADMIN>    # admin-only
```

Run at least weekly (more often on fast ledgers); automate alongside `refresh`. A
missed extension can evict storage, making the contract appear uninitialized.

## 8. Monitoring events

Compact typed events. Topics are indexed; alert on anything unexpected.

**Runtime**

| Event | Topics | Payload | Meaning / response |
|-------|--------|---------|--------------------|
| `RefreshSuccess` | asset | mantissa, expo, timestamp | price accepted, cache updated |
| `RefreshFailure` | asset | code | failed refresh — 1 aggregation/quorum, 2 breaker error, 3 storage (missing key; maybe TTL-evicted), 4 conversion overflow, 5 all sources down, 6 unknown asset |
| `CacheBlocked` | asset | reason_code | valid price blocked — 1 manual, 2 automatic breaker |
| `CircuitBreakerConfigSet` | asset | sample_interval_secs, history_len | breaker set reconfigured |
| `CircuitBreakerAdded` | asset, breaker_id | breaker_kind (1/2/3) | breaker added |
| `CircuitBreakerRemoved` | asset, breaker_id | — | breaker removed; state cleared, cache invalidated |
| `CircuitBreakerEnforcementSet` | asset, breaker_id | is_enforced | enforcement toggled |
| `CircuitBreakerRearmed` | asset, breaker_id | armed_after_secs, accepted_history_source_code (0/1) | breaker rearmed |
| `CircuitBreakerTripped` | asset, breaker_id | tripped_at_secs, price, timestamp, is_enforced | automatic trip; blocks iff `is_enforced` |
| `ManualTripSet` | asset | is_manually_tripped, metadata | governed trip/untrip — correlate the operator via the governance proposal |
| `ProxySet` | asset | source_count, min_sources | proxy config set; cache invalidated |
| `ProxyRemoved` | asset | — | proxy + all state cleared; downstream now reads `None` |
| `ContractUpgraded` | — | new_wasm_hash | runtime code swapped — high impact, verify |
| `TtlExtended` | — | asset_count | runtime `extend_ttl` ran |

The runtime also emits `stellar_access` ownership events on transfer / accept /
renounce.

**Governance**

| Event | Topics | Payload | Meaning / response |
|-------|--------|---------|--------------------|
| `ProposalSubmitted` | id | valid_after_ns, action_code | proposal queued; do not execute before `valid_after_ns` |
| `ProposalAccepted` | id | — | proposal executed; confirm the matching runtime event fired |
| `ProposalRevoked` | id | — | proposal cancelled without executing |
| `OwnershipTransferSubmitted` | id, new_owner | — | a `TransferOwnership` proposal exists — alert and verify `new_owner` before maturity |
| `ActionTtlSet` | — | kind, new_ttl_ns | per-kind TTL changed; a shorter TTL shrinks the catch/revoke window |
| `TtlExtended` | — | — | governance `extend_ttl` ran |

## 9. Incident — manual trip / untrip

Trip a feed when you suspect manipulation or compromise that breakers have not
caught. Trip and untrip both need the `ManualTripper` role (or `Admin`); metadata
is event-only (≤ 1024 bytes).

```bash
# trip (set false to untrip); execute after the SetManualTrip maturity delay
inv --id $GOV -- create_proposal --caller <OP> --id <NEXT> --requested_ttl 0 \
  --operation '{"SetManualTrip": [{"Other":"BTC"}, true, "<reason ≤1024B>"]}'
inv --id $GOV -- execute_proposal --caller <OP> --id <ID>
inv --id $RT  -- get_breaker_set_view --asset '{"Other":"BTC"}'   # is_manually_tripped / is_blocking
```

A trip invalidates the cache immediately; `aggregated_latest` and adapter
`lastprice` return `None` until untripped and refreshed. After untripping, call
`refresh` before downstream services resume.

## 10. Incident — source outage

`RefreshFailure code 5` = all sources down; `code 1` = fewer than `min_sources`
responded. If quorum is still met, no action is needed. Otherwise: manually trip
the feed to make the failure explicit, or propose lowering `min_sources` /
adding a backup source. The cache holds the last accepted price until
`max_age_secs` elapses, after which reads fail closed.

## 11. Upgrade

Run the release gate and dry-run on the new code first; cross-check the manifest
SHA-256 against the installed artifact. Zero WASM hashes are rejected; there is
no `AdminFunctionCall`.

```bash
stellar contract install --network $NET --source $SRC --wasm <new>.optimized.wasm   # returns <HASH>

# runtime — direct (governance authorizes) or via the Upgrade proposal action:
inv --id $RT --source <gov-signer> -- upgrade --new_wasm_hash <HASH> --operator $GOV
# or: create_proposal {"Upgrade":"<HASH>"} (Admin) → execute_proposal after maturity

# adapter — owner-gated:
inv --id $AD -- upgrade --new_wasm_hash <HASH> --operator <OWNER>
```

## 12. Rollback

Roll back within the first 30 minutes if any of these appear: `RefreshFailure`
for previously-healthy assets; `aggregated_latest` / adapter `lastprice`
returning `None` for assets that had accepted prices; governance proposals
failing to submit/execute/cancel; `extend_ttl` erroring; an unexpected ownership
transfer; or an optimized WASM over budget.

Procedure: install the previous WASM (identify it by the SHA-256 in the prior
manifest), upgrade back to its hash, and verify `refresh` succeeds. Keep the
previous manifest and artifacts until the new version has been stable for 48
hours.
