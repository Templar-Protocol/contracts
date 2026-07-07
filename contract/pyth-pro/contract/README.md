# templar-pyth-pro-adapter-contract

NEAR cdylib for the Pyth Pro adapter: stores verified prices and serves them by native Lazer
`u32` feed id. Verification lives in `templar-pyth-pro-verifier`; see the
[adapter overview](../README.md).

## Methods

Init / config:
- `new(owner, config)` — `config`: `signers`, `max_timestamp_delay_s`, `max_timestamp_ahead_s`,
  `allowed_channel_id`, `update_fee` (a `NearToken`, default `0`), `max_feeds_per_update` (non-zero
  cap on feeds accepted per `update_price_feeds` call, bounding the `UpdatePrices` event / log size).
- `get_config()`.

Owner-only (`admin_*`, `#[payable]`, 1 yocto):
- `admin_set_config(config)`.
- `admin_set_signer(public_key: hex, expires_at_s: Option<u64>)` — add/refresh (`Some`) or remove
  (`None`) a 32-byte ed25519 signer (64 hex chars).
- `admin_withdraw(amount: NearToken)` — send accrued fees/free balance to the owner (the runtime's
  storage-staking guard blocks withdrawing below the staked requirement).
- `admin_upgrade(code: Base64VecU8, migrate_args: Base64VecU8)` — atomically deploy new contract
  code and run its `migrate` in one receipt: a failed migration reverts the code deploy too.
  `migrate_args` is the JSON-encoded migration selector. The contract launches at state version 1
  with no migrations defined, so this is the seam for future version bumps; the batched `migrate` is
  private (only the runtime, acting as this account, can call it).

State versioning: persistent state is wrapped in `VersionedState` (launches at version 1). View
helpers `get_stored_state_version()`, `get_target_state_version()`, and `needs_migration()` report
the on-chain vs. code-target state version — `needs_migration()` is `false` on a fresh deploy and
guards against accidental downgrades (a stored version newer than the code panics).

Permissionless write (`#[payable]`):
- `update_price_feeds(payload: Base64VecU8)` — verify and store; emits `UpdatePrices`. Bundles
  carrying more than `config.max_feeds_per_update` feeds are rejected up front (keeps the emitted
  event/log bounded). A feed is stored only with both a price and an exponent, only if its effective
  per-feed publish timestamp strictly advances (anti-replay) and is not too far in the future. EMA
  (price + strictly-positive confidence) is **required**: a spot-only payload is rejected wholesale, so it
  can never overwrite a stored feed and drop its EMA. EMA is never derived from spot. (The stateless
  `verify_update` view does not require EMA — see below.) The caller must attach a deposit covering
  the newly consumed storage plus `config.update_fee`; the excess is refunded. Updates that only
  overwrite known feeds consume no new storage, so with a zero fee they are effectively free.

Storage policy: every verified feed is stored by its native Lazer `u32` id; the submitter funds the
storage. There is deliberately no allowlist.

Feed-id read ABI (the form the proxy-oracle's `Lazer` source calls, addressing feeds by native
`u32` id): `get_feeds_data(feed_ids) -> {feed_id -> Option<FeedData>}` (bulk) and
`get_feed_data(feed_id) -> Option<FeedData>` (single). The adapter is a pure store-and-serve
oracle: it returns the raw stored `FeedData` (spot, EMA, exponent, publish time) and the consumer
projects it to a Pyth price itself (via `FeedData::to_ema_price` / `to_pyth_price` in
`templar-common`) and applies its own freshness policy — mirroring the RedStone adapter. There is
no `PriceIdentifier`-keyed read surface and no on-read age filtering: the `PriceIdentifier ↔ feed_id`
mapping and freshness both live with the proxy-oracle, not this adapter.

Stateless verify (read-only): `verify_update(payload) -> VerifiedUpdateView` runs the full
verification and returns the **complete** Lazer data for every feed (all properties — not just the
Pyth subset) **without** writing storage or charging a fee. It's the official-Lazer-style parity
surface for off-chain RPC (`near view`) and async on-chain callers; it does not replace the
store+serve path above (NEAR has no synchronous cross-contract reads).

## Layout

- `lib.rs` — state (`config`, `feeds: u32 -> FeedData`), init, admin, write path, feed-id read ABI
  (`get_feeds_data` / `get_feed_data`). `FeedData` itself lives in `templar-common` (`oracle::lazer`).
- `crypto.rs` — `Crypto` via `env::ed25519_verify`.  `events.rs` — `UpdatePrices` event.

## Build

Build the deployable artifact with `--target wasm32-unknown-unknown`; run the integration tests on
host with `cargo test`.
