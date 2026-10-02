# Soroban Proxy Oracle — Operational Runbook

Deploy, configure, monitor, respond to incidents, upgrade, and roll back.
Companion docs: `README.md` (overview), `PARITY.md` (NEAR parity), `AUDIT.md`
(boundary).

## Conventions

Examples assume these are exported, and use the `inv` helper for brevity:

```bash
export NET=<network> SRC=<identity>                 # network + signing identity
export RT=<runtime_id> GOV=<governance_id> AD=<adapter_id> LZ=<lazer_source_id> BATCH=<batcher_id>
JF=contract/proxy-oracle/soroban/justfile
inv() { stellar contract invoke --network "$NET" --source "$SRC" "$@"; }
```

So `inv --id "$GOV" -- next_proposal_id` runs a view. Never put key material in
scripts or logs.

## 1. Build and release gates

```bash
just -f $JF test
just -f $JF test-integration
just -f $JF test-scripts
just -f $JF size-check       # developer build; invalidates release evidence
just -f $JF release-gate     # clean tracked tree only
```

`release-gate` performs one fresh unoptimized+optimized build of each of the
five artifacts, publishes the optimized files, writes the schema-4 manifest
last, and validates the exact files without rebuilding. A failed build or
validation removes stale manifest/PASS evidence. `validate-release` takes no
paths: it validates only the canonical manifest and artifacts under
`target/proxy-oracle-soroban/`.

Size budgets: runtime and governance ≤ 131072 bytes; adapter, Lazer source, and
batcher ≤ 32768 bytes. The manifest also binds the clean Git commit, Stellar CLI
and Rust versions, package versions, canonical paths, Wasm SHA-256, reviewed
contract-spec SHA-256, and size policy for all five artifacts.

## 1b. Live end-to-end rehearsal

The rehearsal is testnet-only. It snapshots a validated clean-source release,
pins provider code and every behavioral input, and records signed envelopes,
transaction hashes, independent RPC results, postconditions, and phase outcomes
under `target/proxy-oracle-soroban/e2e/testnet/`:

```bash
SRC=<funded-cli-identity> PYTH_LAZER_API_KEY_FILE=<mode-600-file> \
  contract/proxy-oracle/soroban/scripts/e2e_live.sh all
```

Phases are `deploy`, `configure`, `push`, and `refresh`; a later phase refuses
to run before its prerequisites pass. Re-running re-hashes and resumes the same
checkpoint envelope and refuses administrator, deterministic deployment plan,
artifact, provider, tool, policy, or endpoint drift. Use `--reinitialize` only
when a new deployment plan is intentional; reset refuses an unmarked non-empty
output directory. This tool has no mainnet mode and never tears down contracts.

## 2. Deploy

Upload each optimized WASM and record the returned hash:

```bash
stellar contract upload --network $NET --source $SRC \
  --wasm target/proxy-oracle-soroban/wasm/<artifact>.optimized.wasm
```

The five artifacts are runtime, governance, SEP-40 adapter, Pyth Lazer source,
and batcher. For a release deployment, hashes and bytes must match the validated
schema-4 manifest; do not substitute a developer `build`/`optimize` output.

## 3. Initialize

Constructors are one-shot (`AlreadyInitialized` on re-call). The runtime takes
its owner and governance takes the runtime, so deploy the runtime with a
bootstrap owner first and hand it to governance once governance exists:

```bash
stellar contract deploy --network $NET --source $SRC --wasm-hash <RUNTIME_HASH> -- \
  --governance <ADMIN> --base '{"Other":"USD"}'          # bootstrap owner = ADMIN
stellar contract deploy --network $NET --source $SRC --wasm-hash <GOV_HASH> -- \
  --admin <ADMIN> --proxy_oracle $RT \
  --initial_uniform_ttl_ns 86400000000000                # 24h, uniform across all OperationKinds
```

- `base` is the source-validation invariant — every source's `base()` must match
  it. Per-feed `decimals`/`resolution` are adapter-side, not here.
- Tune per-kind maturity later with `SetActionTtl`.

Hand the runtime's owner to governance via the two-step `Ownable` transfer:

```bash
inv --id $RT -- transfer_ownership --new_owner $GOV --live_until_ledger <MAX_TTL_LEDGER>
# $GOV is now the pending owner. Finalize through governance — it dispatches
# accept_ownership on the runtime; every later owner-only call goes the same way.
inv --id $GOV -- create_proposal --caller <ADMIN> --id <NEXT> --requested_ttl 0 \
  --operation '"AcceptOwnership"'                # Admin-only; void action, no payload
inv --id $GOV -- execute_proposal --caller <ADMIN> --id <ID>
inv --id $RT -- get_owner          # verify == $GOV
inv --id $GOV -- proxy_oracle      # verify == $RT
```

Deploy one adapter per feed (`decimals ≤ 18`, `resolution ≠ 0`):

```bash
stellar contract deploy --network $NET --source $SRC --wasm-hash <ADAPTER_HASH> -- \
  --owner <OWNER> --parent_oracle $RT --asset '{"Other":"BTC"}' \
  --decimals 8 --resolution 1 --base '{"Other":"USD"}'
```

Deploy the Pyth Lazer source with an explicit bounded feed registry (mainnet
verifier `CACZ3GBAKUPIAFRILUFO27J5RUH5GJ2VSJ46LP6GJYSKGDRTQ5MS3HCH`, testnet
`CAYFT5JE3UQTKT4Q6ZOZK4FXVYVT6RE3MFC7STA4UB6WAEGBT65MRU52`) and deploy the
stateless batcher with no arguments. Admitted feeds are served under
`{"Other":"<feed id>"}` (XLM 23, USDC 7, EURC 240); the governed proxy
configuration maps those keys to protocol assets:

```bash
stellar contract deploy --network $NET --source $SRC --wasm-hash <LAZER_HASH> -- \
  --owner <OWNER> \
  --config '{"verifier":"<PYTH_VERIFIER>","base":{"Other":"USD"},"decimals":8,
             "channel":"FixedRate200ms",
             "freshness":{"max_age_secs":120,"max_clock_drift_secs":5}}' \
  --supported_feed_ids '[23]'
stellar contract deploy --network $NET --source $SRC --wasm-hash <BATCHER_HASH>
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
| 6 | `RenounceOwnership` | Admin |
| 7 | `SetManualTrip(asset, tripped, metadata)` | ManualTripper |
| 8 | `AcceptOwnership` | Admin |
| 9 | `TransferOwnership(new_owner)` | Admin |
| 10 | `SetActionTtl(kind, new_ttl_ns)` | ProxyConfigurationManager |
| 11 | `SetRole(account, role, set)` | Admin |
| 12 | `Upgrade(new_wasm_hash)` | Admin |
| 13 | `Rearm(asset, breaker_id, config)` | CircuitBreakerOperator |
| 14 | `SetEnforced(asset, breaker_id, config)` | CircuitBreakerOperator |

## 5. Configure sources, breakers, and roles

Wrap each action in `create_proposal` + `execute_proposal` (examples show only
the `--action` JSON).

**Sources** — `SetProxy` / `RemoveProxy` (mainnet ids; on testnet use Reflector
`CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63`, RedStone
`CA7MY6TYNL5Z5H5FYGMN7YWSY3JIZG7LFY3DZ26EEGRBQ2UKTFWHD4ZJ` with its XLM SAC
`CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`):

```json
{"SetProxy": [{"Other":"XLM"}, {
  "sources": [
    {"oracle":"CAFJZQWSED6YAWZU3GWRTOCNPPCGBN32L7QV43XX5LZLFTK6JLN34DLN",
     "asset":{"Other":"XLM"},"max_age_secs":600,"max_clock_drift_secs":60},
    {"oracle":"CBMGLKUQZVSAIL5CPDDAWSUY7MAKXISHMOZEVLMBUWBMFGHRJSR4WYRF",
     "asset":{"Stellar":"CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA"},
     "max_age_secs":46800,"max_clock_drift_secs":60},
    {"oracle":"<LZ>","asset":{"Other":"23"},
     "max_age_secs":600,"max_clock_drift_secs":60}
  ],
  "min_sources":3,
  "max_cache_age_secs":600
}]}
```

Configure 3–16 distinct oracle addresses and `min_sources ∈ [3, n]`.
Freshness is per source: `max_age_secs` is capped at seven days and
`max_clock_drift_secs` at one hour. `max_cache_age_secs` independently caps the
accepted proof. Reflector keys by symbol, RedStone by SAC address, and Lazer by
feed id. Reflector timestamps use 300-second buckets; RedStone updates on 0.2%
deviation or a 12-hour heartbeat. With `min_sources = 3`, one filtered source
fails a three-source refresh, so choose each window deliberately and use
breakers to constrain the wider one.

Any changed `SetProxy` policy clears accepted history and cache.
Freshness/cache-only changes retain breaker state; changing the ordered
`(oracle, asset)` pairs or `min_sources` also clears breakers. `RemoveProxy`
clears all state.

**Breakers** — configure the set, add a breaker, then enforce it:

```json
{"ConfigureBreakers": [{"Other":"BTC"}, 60, 16]}                          // sample_interval_secs, history_len (1–32)
{"AddBreaker": [{"Other":"BTC"}, {"StepwiseChange": {"max_relative_change":"<hex>"}}]}
{"SetEnforced": [{"Other":"BTC"}, <BREAKER_ID>, {"is_enforced": true}]}
{"Rearm": [{"Other":"BTC"}, <BREAKER_ID>, {"arming_delay_secs": 3600}]}
```

Kinds: `StepwiseChange` (1, sudden jumps), `MonotonicRun` (2, staged ramps),
`WindowedChangeDelta` (3, window-mean drift), and `CumulativeChange` (4,
change from its immutable accepted baseline). Unenforced breakers still evaluate
but a trip does not block the feed. `history_len` must cover every installed
rule; undersized configurations are rejected. `MonotonicRun` requires
`sample_interval_secs = 0` so every accepted step is evaluated. Rearm preserves
both shared histories. To recover any breaker after a sustained legitimate move,
disable enforcement, refresh an accepted price, rearm, then re-enable enforcement.
To intentionally establish a new cumulative baseline, remove the breaker,
complete a successful refresh, then add it again. Every persisted breaker set is
semantically valid. An invalid stored set is unreachable; recover from genuine
corruption with `remove_proxy` followed by `set_proxy`.

**Roles** — `SetRole(account, role, set)` for `Admin`, `ManualTripper`,
`CircuitBreakerOperator`, `ProxyConfigurationManager`. The last `Admin` cannot be
revoked. Inspect on governance: `has_role --account --role`, `list_role --role`,
`get_roles --account`.

## 6. Refresh cadence

`refresh` is the only path that reads source contracts; all other reads are
storage-only.

```bash
inv --id $RT -- refresh --asset '{"Other":"BTC"}'
# or every asset in one operation:
inv --id $BATCH -- refresh_many --oracle $RT --assets '[{"Other":"XLM"},{"Other":"USDC"}]'
```

A Lazer-backed proxy needs the source fed first: fetch a `leEcdsa`-format update
covering every feed in use (one subscription, one payload), requesting the
`price`, `exponent` and `feedUpdateTimestamp` properties — the source skips any
feed missing one of them — and push it with
`inv --id $LZ -- update_price_feeds --payload <hex>`; the return value is the
number of feeds stored (0 means nothing advanced or nothing qualified).

Returns `RefreshStatus`: `Accepted`, `Blocked`, `ResolveFailed`,
`SourceUnavailable`, or `UnknownAsset`. The accepted cache persists the minimum
of `max_cache_age_secs` and all admitted source deadlines; reads use that stored
`valid_until`, not current policy.

Every candidate is evaluated by breakers before its publication time can
advance history. An equal non-advancing candidate may refresh its proof. A
different non-advancing candidate serves the prior price only while the prior
proof is still live and never extends it; without a live proof it returns
`ResolveFailed(7)`. Failed or blocked refreshes replace the accepted status.
`aggregated_history` remains a monotonic source-time audit trail but is hidden
while a manual or enforced breaker blocks the asset.

## 7. TTL extension

```bash
inv --id $RT  -- extend_ttl --asset '{"Other":"BTC"}'
inv --id $GOV -- extend_ttl
# or batched:
inv --id $BATCH -- extend_ttl_many --oracle $RT --assets '[{"Other":"XLM"},{"Other":"USDC"}]'
inv --id $BATCH -- extend_ttl_contracts --contracts '["'$GOV'","'$AD'","'$LZ'"]'
```
Every TTL entrypoint is permissionless: the invoker pays the transaction fee, but no role or authorization is required. The batcher also renews each target's (and its own) instance and code entries, so the WASM code cannot be archived out from under live instances.

The runtime accepts only registered assets. Once the proxy exists, it renews every
surviving per-asset key independently; a missing cache, history, breaker set, or
asset registry does not prevent maintenance.

## 8. Monitoring events

Compact typed events. Topics are indexed; alert on anything unexpected.

**Runtime**

| Event | Topics | Payload | Meaning / response |
|-------|--------|---------|--------------------|
| `RefreshSuccess` | asset | mantissa, expo, timestamp | source-time price accepted for cache/history |
| `RefreshEvaluated` | asset | mantissa, expo, timestamp | candidate accepted by breakers but not advanced; paired with the served `RefreshSuccess` |
| `RefreshFailure` | asset | code | failed refresh — 1 aggregation/quorum, 3 internal storage, 5 all sources down, 6 unknown asset, 7 non-advancing candidate without live proof |
| `CacheBlocked` | asset | reason_code | valid price blocked — 1 manual, 2 automatic breaker |
| `CircuitBreakerConfigSet` | asset | sample_interval_secs, history_len | breaker set reconfigured |
| `CircuitBreakerAdded` | asset, breaker_id | breaker_kind (1/2/3/4) | breaker added |
| `CircuitBreakerRemoved` | asset, breaker_id | — | breaker removed; state cleared, cache invalidated |
| `CircuitBreakerEnforcementSet` | asset, breaker_id | is_enforced | enforcement toggled |
| `CircuitBreakerRearmed` | asset, breaker_id | armed_at_secs | breaker rearmed |
| `CircuitBreakerTripped` | asset, breaker_id | tripped_at_secs, price, expo, publish_timestamp_secs, is_enforced | automatic trip; blocks iff `is_enforced` |
| `ManualTripSet` | asset | is_manually_tripped, metadata | governed trip/untrip — correlate the operator via the governance proposal |
| `ProxySet` | asset | source_count, min_sources | changed ordered `(oracle, asset)` pairs or `min_sources` clear breaker state, history, and cache; other config changes clear history and cache |
| `ProxyRemoved` | asset | — | proxy + all state cleared; downstream now reads `None` |
| `ContractUpgraded` | — | new_wasm_hash | runtime code swapped — high impact, verify |
| `TtlExtended` | asset | — | runtime `extend_ttl(asset)` ran |

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

`RefreshFailure code 5` = all sources down while unblocked; `code 1` = fewer
than `min_sources` positive-weight sources responded; and `code 2` = a breaker
rejected the aggregate. A pre-existing manual or enforced breaker reports
`CacheBlocked` instead. Every failure status replaces the cached result,
including source and quorum failures, so downstream reads fail closed until a
later accepted refresh.

## 11. Upgrade

Run the clean-source release gate first and cross-check the validated manifest
SHA-256 against the installed artifact. Zero WASM hashes are rejected; there is
no `AdminFunctionCall`.

```bash
stellar contract upload --network $NET --source $SRC --wasm <new>.optimized.wasm   # returns <HASH>

# runtime — direct (governance authorizes) or via the Upgrade proposal action:
inv --id $RT --source <gov-signer> -- upgrade --new_wasm_hash <HASH> --operator $GOV
# or: create_proposal {"Upgrade":"<HASH>"} (Admin) → execute_proposal after maturity

# adapter and Lazer source — owner-gated, same shape:
inv --id $AD -- upgrade --new_wasm_hash <HASH> --operator <OWNER>
inv --id $LZ -- upgrade --new_wasm_hash <HASH> --operator <OWNER>

# batcher — stateless and ownerless, so it is replaced rather than upgraded:
stellar contract deploy --network $NET --source $SRC --wasm-hash <BATCHER_HASH>   # returns new $BATCH
# then point the keeper at the new address; the old instance simply expires.
```

The Lazer source's stored prices survive its upgrade (persistent storage is
untouched); a new version must keep reading `StoredPrice` as stored. Rolling it
back is the same call with the previous hash.

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
