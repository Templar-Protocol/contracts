# templar-pyth-pro-adapter-contract

NEAR cdylib for the Pyth Pro adapter: stores verified prices and serves them through the
`pyth-oracle.near` view ABI. Verification lives in `templar-pyth-pro-verifier`; see the
[adapter overview](../README.md).

## Methods

Init / config:
- `new(owner, config)` — `config`: `signers`, `max_timestamp_delay_s`, `max_timestamp_ahead_s`,
  `allowed_channel_id`, `update_fee` (a `NearToken`, default `0`), `default_valid_time_period_s`
  (staleness window for the non-suffixed views).
- `get_config()`.

Owner-only (`admin_*`, `#[payable]`, 1 yocto):
- `admin_set_config(config)`.
- `admin_set_signer(public_key: hex, expires_at_s: Option<u64>)` — add/refresh (`Some`) or remove
  (`None`) a 32-byte ed25519 signer (64 hex chars).
- `admin_set_feed_mapping(price_identifier, feed_id: Option<u32>)` — map/unmap. Unmapping removes
  only the id→feed entry; `feeds[feed_id]` is retained, so remapping can resurface pre-existing data
  via the `*_unsafe` views / `price_feed_exists` (the `*_no_older_than` views age-filter it). Re-push
  an update after (re)mapping if that matters.
- `admin_withdraw(amount: NearToken)` — send accrued fees/free balance to the owner (the runtime's
  storage-staking guard blocks withdrawing below the staked requirement).

Permissionless write (`#[payable]`):
- `update_price_feeds(payload: Base64VecU8)` — verify and store; emits `UpdatePrices`. A feed is
  stored only with both a price and an exponent, only if its effective per-feed publish timestamp
  strictly advances (anti-replay) and is not too far in the future, and EMA is stored only when the
  payload actually carried it (never derived from spot). The caller must attach a deposit covering
  the newly consumed storage plus `config.update_fee`; the excess is refunded. Updates that only
  overwrite known feeds consume no new storage, so with a zero fee they are effectively free.

Storage policy: every verified feed is stored, **regardless of whether a consumer `PriceIdentifier`
currently maps to it**. This is intentional — it keeps update correctness independent of the
removable `feed_map` seam. The submitter funds the storage, and unmapped feeds are simply not
queryable until an `admin_set_feed_mapping` exists. There is deliberately no mapped-only allowlist.

Pyth views (drop-in for `pyth-oracle.near`): `price_feed_exists`; spot `get_price` /
`get_price_unsafe` / `get_price_no_older_than` and the `list_prices*` forms; EMA `get_ema_price` /
`get_ema_price_unsafe` / `get_ema_price_no_older_than` and the `list_ema_prices*` forms. The
non-suffixed `get_price` / `get_ema_price` / `list_prices` / `list_ema_prices` apply
`config.default_valid_time_period_s` as their staleness window. Plus `get_feed_mapping`,
`list_feed_mappings`, and `get_feed_data(feed_id)` (debug).

Stateless verify (read-only): `verify_update(payload) -> VerifiedUpdateView` runs the full
verification and returns the **complete** Lazer data for every feed (all properties — not just the
Pyth subset) **without** writing storage / charging a fee / touching mappings. It's the
official-Lazer-style parity surface for off-chain RPC (`near view`) and async on-chain callers; it
does not replace the store+serve path above (NEAR has no synchronous cross-contract reads).

## Layout

- `lib.rs` — state (`config`, `feeds: u32 -> FeedData`, `ids`), init, admin, write path.
- `crypto.rs` — `Crypto` via `env::ed25519_verify`.
- `feed_map.rs` — **removable** `PriceIdentifier ↔ u32` seam.
- `views.rs` — Pyth read ABI.  `events.rs` — `UpdatePrices` event.

## Build

Build the deployable artifact with `--target wasm32-unknown-unknown`; run the integration tests on
host with `cargo test`.
