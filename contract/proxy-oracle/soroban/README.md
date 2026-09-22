# Soroban Proxy Oracle

Aggregates external SEP-40 price feeds into a normalized, exponent-form cache. A companion `Sep40Adapter` contract re-exposes the cached prices as SEP-40 `PriceFeedTrait` for downstream consumers at per-adapter `decimals` / `resolution` / `base`. `PythLazerSource` turns Pyth Lazer's stateless on-chain verifier into a SEP-40 source the runtime can pull, and `ProxyOracleBatcher` fans the permissionless `refresh` / `extend_ttl` calls across assets so a keeper needs one transaction per sweep.

The runtime is **not** itself a SEP-40 contract. It exposes:

- `refresh(asset)` — pull one asset's source prices, aggregate through
  `templar-proxy-oracle-kernel`, apply freshness + breakers, and write the
  resulting status to its cache. The accepted cache persists a `valid_until`
  equal to the earliest admitted-source deadline or cache-age deadline. Equal
  non-advancing observations may refresh that proof. A different
  non-advancing observation can serve the prior price only while its original
  proof remains live and never extends it; otherwise refresh returns
  `ResolveFailed(7)`. Failed and blocked refreshes replace an accepted cache.
  This is the only source-IO path.
- `aggregated_latest(asset) -> Option<NormalizedPrice>` — the most recently
  accepted source-time aggregate `{ mantissa, expo, timestamp }`, or `None`
  after its persisted acceptance deadline or any terminal cache status. Later
  policy changes cannot resurrect an expired proof.
- `aggregated_history(asset, records)` — the last N accepted aggregates with
  strictly increasing publication timestamps, or `None` while a manual or
  enforced automatic breaker blocks the asset. It is a monotonic source-time
  record, not a time-bucket view of `aggregated_latest`.
- Introspection: `registered_assets`, `source_base`, `get_proxy`, `get_cached`,
  `get_breaker_set_view`, `get_owner`.

Any changed proxy policy clears cache and accepted history. Freshness/cache-only
changes retain breaker state; changing the ordered `(oracle, asset)` pairs or
`min_sources` also clears breakers. Reads are storage-only and fail closed.

RedStone enters through its `RedStoneSep40` adapter (`CBMGLKUQZVSAIL5CPDDAWSUY7MAKXISHMOZEVLMBUWBMFGHRJSR4WYRF` on mainnet, assets keyed by SAC address); its published per-feed contracts are Chainlink-shaped, not SEP-40, and cannot be sources. Pyth Lazer enters through `PythLazerSource`. This proxy verifies no oracle payloads itself.

## Governance

The runtime's owner — normally the companion `templar-proxy-oracle-soroban-governance-contract` — is managed by `stellar_access::ownable` (two-step `transfer_ownership` / `accept_ownership` / `renounce_ownership`, plus `get_owner`). Every config mutation is `#[only_owner]`, so the owner must authorize it.

- **Handoff**: an Admin `TransferOwnership(new_owner)` proposal dispatches `transfer_ownership`; the new owner finalizes with `accept_ownership` (directly, or via an `AcceptOwnership` proposal on its own governance contract).
- **Renounce**: `RenounceOwnership` permanently clears the owner; every later `#[only_owner]` call then panics and the config is frozen. No undo.
- **Roles** (`stellar-access` RBAC): `Admin`, `ManualTripper`, `CircuitBreakerOperator`, `ProxyConfigurationManager`. Admin overrides any action; the last Admin cannot be removed. Emergency trips use the `SetManualTrip` action (ManualTripper role).
- **Proposals**: `create_proposal(caller, id, operation, requested_ttl)`, executed by id after maturity via `execute_proposal`; `cancel_proposal` frees a slot. At most 64 pending. Query with `active_ids`, `get_proposal`, `get_operation_ttl`, `get_effective_proposal_ttl`. Per-operation maturity (`OperationKind` / `TtlConfig`) is seeded uniform at construction and adjusted with `SetActionTtl`; `Rearm` and `SetEnforced` carry independent TTLs.
- **Upgrades**: `upgrade(new_wasm_hash, operator)` on the runtime, proposed via the Admin `Upgrade` action. NEAR's `AdminFunctionCall` arbitrary dispatch is intentionally not ported — the upgrade surface stays typed.

The proposal state machine is shared with NEAR via the `no_std` `templar-proxy-oracle-governance-kernel`; each runtime owns its own authorization, storage encoding, and events.

## Sep40Adapter

Each adapter is independently `Ownable` and binds one immutable
`(parent_oracle, asset, decimals, resolution, base)` tuple. It requires the
parent's `source_base` to equal its base at construction and before every price
read. Repointing a feed, relabeling it, or changing output precision requires a
new adapter. Owner entrypoints:

- `decommission()` — permanently disables `price`, `prices`, and `lastprice`;
  call it before `renounce_ownership`.
- `upgrade(new_wasm_hash, operator)` — owner-gated wasm swap; emits
  `AdapterUpgraded`.

`extend_ttl()` is permissionless instance-storage maintenance. `config()` views
the immutable `{ parent_oracle, asset, decimals, resolution, base }`.

`PriceFeedTrait` projects parent prices to the adapter precision and resolution
buckets; unrepresentable values and a parent-base mismatch fail closed. SEP-40
metadata (`contractmeta!(key = "sep", val = "40")`) is declared here, not on the
runtime. The release manifest binds the adapter Wasm, not deployed feed addresses.

## PythLazerSource

Pyth's Lazer contract on Stellar is a stateless verifier: `verify_update(Bytes) -> Bytes`
proves a payload was signed by a trusted signer and returns it, with no replay protection,
ordering, or freshness check. `PythLazerSource` owns those controls and serves
the result as SEP-40, keyed by the Lazer feed id itself: feed 23 is
`Asset::Other("23")`. Which feed backs which proxy asset is decided in the
runtime's governed `SetProxy`, not by this source.

The owner-maintained `supported_feed_ids` registry admits at most 32 active
feeds, keeping a full registry replacement inside Soroban's ledger-write limit,
and makes SEP-40 discovery truthful. Removing a feed deletes its stored
price. Replay watermarks remain allocated across feed removals within an epoch
(bounded at 256). Owner-gated `set_verification_config` and
`reset_verification_epoch` deliberately clear active prices and start a new
replay domain; keepers must repopulate it with freshly verified updates. The
verifier, channel, base, and output decimals are constructor configuration;
decimals/base are not mutable.

- `update_price_feeds(payload)` — permissionless. Verifies through the
  configured verifier, requires the configured channel, then stores only
  admitted feeds whose own update time is inside the freshness window and
  strictly advances. Feeds without a positive price, exponent, or update
  timestamp are skipped. Returns the number stored; `0` is not success evidence
  for a keeper push.
- `lastprice(asset)` rescales stored `(mantissa, expo)` to the constructor
  decimals and keeps second-precision publish time. `resolution` is 1;
  `price` / `prices` serve only the latest record.
- Owner entrypoints: `set_freshness`, `set_supported_feed_ids`,
  `set_verification_config`, `reset_verification_epoch`, and
  `upgrade(new_wasm_hash, operator)`.
- Permissionless `extend_ttl()` renews the instance. Views: `config`,
  `supported_feed_ids`, `verification_epoch`, and `stored_price(feed_id)`.

The payload parser and verifier client are Pyth's own `pyth-lazer-stellar-sdk` 0.3.0, vendored
into the `Templar-Protocol/pyth-lazer-public` fork on soroban-sdk 25 (crates.io 0.3.0 requires
soroban-sdk 26.1 and therefore Rust ≥ 1.91). Swap to the crates.io release once the workspace
toolchain moves.

## ProxyOracleBatcher

Stateless, ownerless. `refresh_many(oracle, assets)`,
`extend_ttl_many(oracle, assets)` and `extend_ttl_contracts(contracts)` forward
the runtime's and sibling contracts' permissionless maintenance calls inside a
single Soroban operation. Every vector is capped at 64 before dispatch;
oversized calls trap with contract error 1 and the ledger atomically applies no
prefix effects. A target trap likewise reverts the operation, while status-level
and caught TTL failures remain visible in the returned vectors. The TTL paths
renew target instance and code entries.

## Operational notes

- Configure 3–16 sources; `min_sources` must be in `[3, sources.len()]`. Invalid quorum is rejected.
- `refresh(asset)` is the only source-IO path; all reads are storage-only.
- Manage breakers with the governed `add_breaker` / `remove_breaker` / `rearm` / `set_enforced`. Inert params and insufficient history are rejected; `MonotonicRun` requires zero sampling, while a `CumulativeChange` baseline is intentionally rebased only by remove → successful refresh → add. Changing an asset's ordered `(oracle, asset)` pairs or `min_sources` clears its breaker set, cache, and history; configure and add breakers again after the source migration. Every persisted breaker set is semantically valid. An invalid stored set is unreachable; recover from genuine corruption with `remove_proxy` → `set_proxy`.
- Manual-trip metadata is event-only, capped at 1024 bytes, not stored in breaker state.
- Schedule an ops/keeper job for Soroban TTL maintenance; do not rely on curators to remember this manually.
- Runtime `extend_ttl(asset)` is permissionless for registered assets and renews every surviving persistent `Proxy`, `Breakers`, `Cache`, and `History` entry.
- Governance `extend_ttl()` is permissionless and renews governance instance state plus active persistent proposal bodies.
- SEP-40 adapter `extend_ttl()` is permissionless and renews adapter instance config. Adapter reads also refresh instance TTL when the remaining TTL is below threshold.
- Keep optimized WASMs within budget: runtime & governance ≤ 128 KiB; adapter,
  Lazer source, and batcher ≤ 32 KiB. `size-check` is a developer gate only;
  release evidence comes from a clean-source five-artifact `release-gate`.
- The XLM testnet rehearsal pins Reflector, RedStone, the Pyth verifier, Lazer
  feed 23, all policy values, provider code hashes, and the exact release
  snapshot in its checkpoint. Resume refuses drift.

## Known limits

- Source contracts must expose the SEP-40 ABI used here. NEAR Pyth sources and NEAR price transformers are not ported.
- Soroban storage is not permanent (unlike NEAR); a missed `extend_ttl` risks eviction. Events are compact typed events, not byte-for-byte equal to NEAR's JSON events.
- Not an in-place migration target for earlier prototype storage layouts — redeploy/reinitialize or ship an explicit migration first.
- **OZ `upgradeable` not adopted**: crates.io v0.7.1 needs Rust ≥ 1.87 (`is_multiple_of`) but the toolchain pins 1.86; the 1.86-compat fork is locked to soroban-sdk 23.x, not the 25.0.1 used here. The hand-rolled `upgrade` is the stopgap until the toolchain bumps or the fork rebases — don't re-investigate without one of those.

## Verification and release

```bash
JF=contract/proxy-oracle/soroban/justfile
just -f $JF test
just -f $JF test-integration
just -f $JF test-scripts
just -f $JF size-check       # developer build; no release claim
just -f $JF release-gate     # clean tracked tree; exact five-Wasm release
```

`release-gate` builds all five Wasms fresh, publishes the optimized files and
schema-4 manifest last, then validates byte/spec hashes, package/tool versions,
canonical paths, and size limits without rebuilding. Developer `build`,
`optimize`, and `size-check` invalidate prior PASS evidence and never emit a
release manifest.

The live proof is testnet-only and consumes that validated release:

```bash
SRC=<funded-cli-identity> PYTH_LAZER_API_KEY_FILE=<mode-600-file> \
  contract/proxy-oracle/soroban/scripts/e2e_live.sh all
```

The mode-0700 checkpoint directory stores immutable artifact snapshots,
deterministically revalidated contract plans, signed envelopes, transaction
hashes, independent RPC results, postconditions, and terminal phase outcomes.
Re-running re-hashes and resumes one unresolved envelope by its transaction
hash. Use `--reinitialize` only to deliberately discard that state and plan new
contract IDs; destructive reset refuses an unmarked non-empty output directory.

All five contracts build through `stellar contract build` with the pinned
toolchain. Plain `cargo build` is not the release path.
