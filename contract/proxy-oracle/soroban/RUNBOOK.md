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

`release-gate` exports the tracked commit outside the checkout, so it does not
inherit untracked checkout Cargo configuration; `TMPDIR` must be outside the
checkout too. It builds and optimizes each of the five artifacts once, publishes
the optimized files, writes the schema-4 manifest last, and validates without
rebuilding. Build attempts invalidate the manifest and both validation reports;
each validation command clears its own prior report before checking.
Checkout and Cargo-home paths are remapped to stable prefixes, and Wasms retaining
either host prefix are rejected. `validate-release` takes no paths: it validates
only the canonical manifest and artifacts under `target/proxy-oracle-soroban/`.
`dry-run` uses the same schema-4 validation and prints upload templates for all
five artifacts. It performs no network or transaction simulation and broadcasts
nothing.

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

Phases are `deploy`, `ownership`, `configure`, `push`, and `refresh`; prerequisites
must pass before a later phase runs. Completed phases are skipped, but while
`refresh` is pending, `push`, `refresh`, and `all` recheck Lazer freshness and
renew stale data without repeating deployment or governance. Fresh-data and
completed-refresh resumes need no Pyth credential; renewing stale data needs it.
Re-running re-hashes and resumes the same checkpoint envelope and refuses
administrator, deployment plan, artifact, provider, tool, policy, or endpoint drift.
Use `--reinitialize` only when a new deployment plan is intentional; reset refuses
an unmarked non-empty output directory. This tool has no mainnet mode and never
tears down contracts.

Each recorded push retains its request/response at
`operations/NNNN/lazer_response.json`. Renewal preparation uses the next operation
directory, so a failed build, simulation, or signing step preserves the previous
push's evidence and its historical phase proof.

A submitted transaction is only resolved by polling its recorded hash, never
resent. A local send error, `PENDING`, or `NOT_FOUND` does not prove rejection:
the operation remains unresolved and blocks new writes until a terminal RPC
result. There is no abandonment command; checkpoints containing an `abandoned`
operation are rejected rather than permitting a replacement.

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
- The bootstrap Admin must authorize the constructor invocation. A newly granted
  Admin must co-authorize its `SetRole` execution; governance itself cannot be an
  Admin. Membership count alone does not establish a usable recovery signer.

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

An executed `TransferOwnership(Address)` retains the maximum acceptance window.
To cancel it, queue `{"CancelOwnershipTransfer":"<PENDING_OWNER>"}` and execute
after the existing **TransferOwnership** timelock. This is Admin-only, requires
the matching pending owner, and also permits cancellation of an expired but
still-stored transfer. It does not emit a transfer-start event.

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
- Queue-full returns `TooManyPendingProposals = 12`; delegates may occupy 63
- Proposal XDR bytes are also bounded in aggregate (122,880 bytes total,
  2,048 reserved for Admin recovery); exceeding the bound returns
  `PendingProposalBytesExceeded = 13` without reserving the ID or mutating
  state. Large `SetProxy` bodies therefore meet backpressure before the queue
  fills, keeping atomic revocation inside host resource limits.
- Queries: `next_proposal_id`, `active_ids`, `get_proposal --id`,
  `get_operation_ttl --kind`, `get_effective_proposal_ttl --operation --requested_ttl`.
- Persist the explicit ID and exact payload before submitting. An uncertain send
  must reconcile its recorded transaction hash, not fetch a replacement ID.
  Stale-ID rejection does not prove this caller succeeded: another caller may
  have consumed it. If recorded evidence or a matching live proposal cannot
  establish the outcome, report unknown and stop; do not execute a guessed body.
  Rerunning an auto-selecting command is a new request, not a safe retry.

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

**Sources** — `SetProxy` / `RemoveProxy`. Resolve actual source addresses and
query each source's base, asset key, and decimals before preparing the proposal.
Placeholders below are not deployment identities. These are three alternative
configurations, not three operations to apply in sequence:

```json
{"SetProxy":[{"Other":"BTC"},{
  "aggregator":{"MedianLow":{
    "sources":[
      {"oracle":"<SRC1>","asset":{"Other":"BTC"},"expected_decimals":8},
      {"oracle":"<SRC2>","asset":{"Other":"BTC"},"expected_decimals":8}],
    "min_sources":2}},
  "max_age_secs":120,"max_clock_drift_secs":0}]}
{"SetProxy":[{"Other":"BTC"},{
  "aggregator":{"MedianHigh":{
    "sources":[
      {"oracle":"<SRC1>","asset":{"Other":"BTC"},"expected_decimals":8},
      {"oracle":"<SRC2>","asset":{"Other":"BTC"},"expected_decimals":8}],
    "min_sources":2}},
  "max_age_secs":120,"max_clock_drift_secs":0}]}
{"SetProxy":[{"Other":"BTC"},{
  "aggregator":{"Priority":[
    {"oracle":"<SRC2>","asset":{"Other":"BTC"},"expected_decimals":8},
    {"oracle":"<SRC1>","asset":{"Other":"BTC"},"expected_decimals":8}]},
  "max_age_secs":120,"max_clock_drift_secs":0}]}
```

Configure 1–16 distinct oracle addresses. Medians have equal unit weights and
`min_sources ∈ [1, n]`; Priority selects the first valid surviving source in
order and has no quorum or weight fields. Different addresses do not prove
independent providers or manipulation resistance. Admission requires the source's
decimals to match its pin (≤ 18); refresh drops changed decimals or base before
conversion. Changing a pin is an explicit aggregation change, not auto-repinning.
Required `u64` freshness bounds are capped at seven days (`max_age_secs`) and one
hour (`max_clock_drift_secs`); zero age is allowed as same-ledger policy.
Reflector keys by symbol, RedStone by SAC address, and Lazer by feed id.
Size freshness for observed provider cadence and keeper interval. Median quorum
fails closed when too few sources survive; Priority fails when all are invalid.

**Size `max_age_secs` for source cadence plus keeper interval, with margin.**
Latest reads require both the cache-update age and aggregate-publication age to
be at most this bound (inclusive; age uses saturating subtraction). A price near
the window edge leaves only its remaining lifetime. Stale reads return `None`,
not fallback data. The same check gates CumulativeChange baseline selection.

Any changed `SetProxy` policy clears accepted history and cache. Freshness-only
changes retain breakers; changing method, ordered `(oracle, asset,
expected_decimals)` sources, or median quorum resets breakers while retaining
the next-ID allocator. Identical configuration is a no-op. `RemoveProxy` rejects
unknown assets and removes served state, retaining an allocator tombstone.
Recreation cannot reuse old breaker IDs; absent-proxy views hide the tombstone.

**Feeds a Blend pool reads: set `max_clock_drift_secs` to 0.** The pool reads this proxy
through the pool oracle adapter, which rounds timestamps down to its resolution, so a
future-dated price that falls in the current bucket passes the pool's own future-date check
(with a resolution of one second, the pool's check still applies). With a non-zero drift, a
source sample published ahead of ledger time can become the newest history entry. The proxy
reads its age as zero until the clock catches up, so `refresh` keeps marking its cache
current, and the pool keeps pricing on that one sample for up to `max_clock_drift_secs`
while the market moves (Guardian L-05). At 0 the freshness filter
rejects any sample published after the current ledger. A cross-chain sample stamped ahead
of the Stellar ledger close time is skipped for that refresh and accepted once ledger time
passes it, usually within one ledger. Check this value on every `SetProxy` for an asset a
pool prices.

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
`sample_interval_secs = 0` so every accepted step is evaluated. Window/streak
rules count samples, not elapsed wall-clock intervals; cadence changes and long
gaps can conservatively trip them. Relative rules cannot manufacture a trusted
first baseline. CumulativeChange installation requires a fresh accepted price.
Rearm preserves both shared histories. `ArmedAfter` intentionally permits
privileged recovery during its cooldown; it is not a fail-closed waiting period.
For a deliberate recovery, review the rule, disable enforcement, refresh, rearm,
and re-enable enforcement. This does not change an immutable cumulative baseline:
to rebase, remove that rule, successfully refresh, then add it again.
Missing/corrupt decoder state is not an empty set and must never silently reset
an unknown allocator. Archived state/tombstones require normal ledger restoration.

**Roles** — `SetRole(account, role, set)` for `Admin`, `ManualTripper`,
`CircuitBreakerOperator`, `ProxyConfigurationManager`. The last `Admin` cannot be
revoked. Inspect on governance: `has_role --account --role`, `list_role --role`,
`get_roles --account`.
Cancellation is creator-or-current-Admin, independent of the creator's remaining
action role; same-role peers may execute but cannot veto each other's proposals.
Revocation cancels that creator's queued operations only when their remaining
roles no longer authorize them. Proposals have no expiry: maturity is snapshotted
at creation, so increasing a timelock does not retroactively delay existing ones.
Delegates can occupy at most 63 of the 64 pending slots; Admin can use the reserve.
This bounds admission but does not guarantee scheduling fairness.
The fresh-stack governance stores unchanged proposal values in bounded persistent
groups so a full-queue revocation can purge atomically within ledger entry limits.
This storage is incompatible with previous governance deployments; no in-place
migration is provided. An archived proposal group requires normal ledger
restoration before cancellation/execution; a missing body never permits silently
dropping the active ID.

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
number of feeds stored (0 means nothing advanced or nothing qualified). A push
that fails simulation reports `Error(Contract, #6)` when Pyth's verifier refused
the signature or envelope, `#3` when the verified bytes would not parse, and
`#7` when the verifier trapped or returned a code the pinned SDK does not map —
investigate `#7` before retrying, it points at the verifier, not the payload.

Returns `RefreshStatus`: `Accepted`, `Blocked`, `ResolveFailed`,
`SourceUnavailable`, or `UnknownAsset`. Reads apply `max_age_secs` from the
asset's current config against the cached entry.

Every candidate is evaluated by breakers before its publication time can
advance history. A non-advancing candidate still serves the prior aggregate and
`RefreshEvaluated` records it when it differs. Failed or blocked refreshes
replace the accepted status.
`aggregated_history` remains a monotonic source-time audit trail but is hidden
while a manual or enforced breaker blocks the asset.
Adapter `prices` projects newest-first distinct resolution buckets, skipping
unconvertible records; parent history remains chronological and
freshness-independent. `price` buckets the requested timestamp and selects the
newest convertible record in that bucket: at resolution 10, history times
`[100, 101, 110]` project to buckets `[110, 100]`, and request 109 resolves 100.

## 7. TTL extension

```bash
inv --id $RT  -- extend_ttl --asset '{"Other":"BTC"}'
inv --id $GOV -- extend_ttl
# or batched:
inv --id $BATCH -- extend_ttl_many --oracle $RT --assets '[{"Other":"XLM"},{"Other":"USDC"}]'
inv --id $BATCH -- extend_ttl_contracts --contracts '["'$GOV'","'$AD'","'$LZ'"]'
```

Also page **every role** (`Admin`, `ManualTripper`, `CircuitBreakerOperator`,
`ProxyConfigurationManager`) through governance:

```bash
inv --id $GOV -- extend_role_ttl --role '"Admin"' --offset 0 --count 16
# If the result is Some(next_offset), repeat for that role with that exact
# returned offset until None, then do the same for each remaining role.
```

Each page permits `1 <= count <= 16` and renews the role count, member indices,
and membership entries. Ordinary governance `extend_ttl` additionally renews
`ExistingRoles`, instance state, and pending proposal bodies; **both** calls are
required. The batcher contract-TTL call includes ordinary governance maintenance
but does not page role members. Membership can change between pages: sweep
periodically rather than assume a transaction-wide membership snapshot.
Every TTL entrypoint is permissionless: the invoker pays the transaction fee, but no role or authorization is required. The batcher also renews each target's (and its own) instance and code entries, so the WASM code cannot be archived out from under live instances.

The runtime accepts only registered assets. Once the proxy exists, it renews every
surviving per-asset key independently; a missing cache, history, breaker set, or
asset registry does not prevent maintenance.

## 8. Monitoring events

Compact typed events. Topics are indexed; alert on anything unexpected.

The companion detail events and history projection below specify the remediation
release surface. Their implementation and integrated verification are required
release gates; this document alone is not evidence that those gates passed.
**Runtime**

| Event | Topics | Payload | Meaning / response |
|-------|--------|---------|--------------------|
| `RefreshSuccess` | asset | mantissa, expo, timestamp | source-time price accepted for cache/history |
| `RefreshEvaluated` | asset | mantissa, expo, timestamp | candidate accepted by breakers but not advanced; paired with the served `RefreshSuccess` |
| `RefreshFailure` | asset | code | 101 aggregation length mismatch, 102 insufficient valid sources, 3 storage failure, 5 source unavailable, 6 unknown asset |
| `AggregationFailureDetail` | asset | code, expected, actual | adjacent to aggregation failure only; required aligned-vector length or source quorum and actual count |
| `CacheBlocked` | asset | reason_code | valid price blocked — 1 manual, 2 automatic breaker |
| `CacheBlockedDetail` | asset | blocking_breaker_ids | adjacent on every blocked refresh, including no-source/already-blocked paths; empty for manual trip |
| `CircuitBreakerConfigSet` | asset | sample_interval_secs, history_len | breaker set reconfigured |
| `CircuitBreakerAdded` | asset, breaker_id | breaker_kind (1/2/3/4) | breaker added |
| `CircuitBreakerAddedDetail` | asset, breaker_id | config, baseline | full typed `CircuitBreakerConfig`; normalized baseline present iff cumulative |
| `CircuitBreakerRemoved` | asset, breaker_id | — | breaker removed; state cleared, cache invalidated |
| `CircuitBreakerEnforcementSet` | asset, breaker_id | is_enforced | enforcement toggled |
| `CircuitBreakerRearmed` | asset, breaker_id | armed_at_secs | breaker rearmed |
| `CircuitBreakerTripped` | asset, breaker_id | tripped_at_secs, price, expo, publish_timestamp_secs, is_enforced | automatic trip; blocks iff `is_enforced` |
| `ManualTripSet` | asset | is_manually_tripped, metadata | governed trip/untrip — correlate the operator via the governance proposal |
| `ProxySet` | asset | source_count, min_sources | median configured quorum, or 1 for Priority; aggregation changes reset breakers preserving allocator, all policy changes clear history/cache |
| `ProxyRemoved` | asset | — | served state removed, allocator tombstone retained; downstream now reads `None` |
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

`RefreshFailure code 5` means no source is available while unblocked; `102`
means aggregation has too few valid sources (inspect companion expected/actual
counts); `101` is an aligned-input length mismatch and requires investigation.
Manual and automatic blocking use `CacheBlocked` reason 1 and 2 respectively,
with exact IDs in `CacheBlockedDetail`. Every failure replaces the cached
result; downstream reads fail closed until a later accepted refresh.

## 11. Upgrade
**This release is fresh-stack-only for runtime/governance.** Unified aggregation
and decimal-pin XDR plus eight fixed proposal groups of up to eight unchanged
bodies are incompatible with previous stacks. In-place migration is skipped;
leave existing deployments unchanged. There is no old decoder or hidden default.
Deploy matching new contracts, prove reciprocal governance/runtime wiring,
complete two-step ownership handoff, authenticate the new Admin, verify adapter
parent/asset/base and parent `source_base`, then harden production timelocks after
deliberate zero-TTL bootstrap. Provider addresses/catalog entries do not establish
live proxy identities.

The calls below describe existing code-swap entrypoints, **not authorization to
install this release over old state**. Governance has no self-upgrade and runtime
has no migrate endpoint. The layout-compatible adapter proof in
`integration-tests/tests/upgrade.rs` mounts the catalog-pinned 0.1.4 artifact and
current optimized Wasm, executes the swap, and preserves Config, ownership and
decommission state while testing corrected history. Run it through
`just -f contract/proxy-oracle/soroban/justfile test-integration`, not the
native-only gate. This is local host evidence, not production upgrade approval.


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

Any independently upgraded Lazer source must preserve decoding of its stored
prices. Prove exact old/new artifact compatibility before either upgrade or
rollback; an unchanged entrypoint alone is insufficient evidence.

## 12. Rollback

Roll back within the first 30 minutes if any of these appear: `RefreshFailure`
for previously-healthy assets; `aggregated_latest` / adapter `lastprice`
returning `None` for assets that had accepted prices; governance proposals
failing to submit/execute/cancel; `extend_ttl` erroring; an unexpected ownership
transfer; or an optimized WASM over budget.

For an independently proven layout-compatible upgrade, identify the previous
artifact by its manifest SHA-256 and use only the tested reverse swap, then
verify reads and refresh. Never install old runtime/governance code over the new
incompatible fresh-stack state. Replacement-stack cutover/rollback requires a
separate operator-authorized plan; keep manifests and exact artifacts.
