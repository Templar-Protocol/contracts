# Oracles

Templar Protocol relies on external price oracles to determine asset valuations when calculating collateralization ratios and performing liquidations.

For a high-level overview of Templar's multi-source proxy oracle architecture, see [Proxy Oracle Architecture](./contract/proxy-oracle/index.md).

[Pyth Network](https://pyth.network/) is the primary oracle provider ([documentation](https://docs.pyth.network/)).

Pyth is a **pull oracle**, meaning that the price feeds are updated as-needed instead of continuously. As such, interactions with Templar markets should always be preceded by a call to the appropriate oracle contract to update the necessary asset prices using a proof provided by Pyth.

More information about how to perform this update on NEAR can be found on [Pyth's documentation site](https://docs.pyth.network/price-feeds/use-real-time-data/pull-integration/near#update_price_feeds).

## Oracle Addresses

| Network | Account ID |
|---------|------------|
| Testnet | [`pyth-oracle.testnet`](https://testnet.nearblocks.io/address/pyth-oracle.testnet) |
| Mainnet | [`pyth-oracle.near`](https://nearblocks.io/address/pyth-oracle.near) |

## Price Identifiers

Price identifiers for Pyth Network assets can be found on [their documentation site](https://docs.pyth.network/price-feeds/price-feeds#feed-ids).

## LST Oracle Adapter

For Liquid Staking Tokens (LSTs), Templar uses [a custom oracle adapter](./contract/lst-oracle.md) ([`lst.oracle.tmplr.near`](https://nearblocks.io/address/lst.oracle.tmplr.near)) to derive the LST price from the underlying asset price(s).

## Proxy Oracle

Templar also supports [proxy oracles](./contract/proxy-oracle/index.md), which present a Pyth-compatible interface to markets while allowing a single market-facing price identifier to be sourced from multiple underlying oracle feeds.

This is the production mechanism used to support backup oracle paths and asset-specific aggregation policies.

## Price Feed Configuration

Each market is [configured](/doc/templar_common/market/struct.PriceOracleConfiguration.html) with the following fields:

```rust
pub struct PriceOracleConfiguration {
    /// Account ID of the oracle contract.
    pub account_id: AccountId,
    /// Price identifier of the collateral asset in the oracle contract.
    pub collateral_asset_price_id: PriceIdentifier,
    /// Collateral asset decimals, to convert the oracle price.
    pub collateral_asset_decimals: i32,
    /// Price identifier of the borrow asset in the oracle contract.
    pub borrow_asset_price_id: PriceIdentifier,
    /// Borrow asset decimals, to convert the oracle price.
    pub borrow_asset_decimals: i32,
    /// Maximum price age to accept from the oracle, after which the price
    /// will be considered stale and rejected.
    pub price_maximum_age_s: u32,
}
```

## Update Frequency and Freshness

- **Update Frequency**: As-needed (pull model)
- **On-Chain Updates**: Pulled on-demand by protocol operations
- **Price Staleness**: Configurable maximum age per market (typically 60 seconds)

## Price Validation

Markets validate price freshness before use. If prices are stale, users must push fresh price data to the oracle contracts

- Operations that require prices (borrow, liquidate) will fail.
- Users must push updates for fresh price data.

Backup oracle support is available through the proxy oracle architecture. A market can continue pointing at the same oracle endpoint while the proxied feed uses one or more underlying sources.

## Oracle Security Measures

- **Confidence Intervals**: Pyth prices include confidence bands. The lower bound is used for collateral valuations, and the upper bound for liability valuations.
- **Multiple Data Sources**: Pyth aggregates from multiple price providers.
- **Time-Weighted Averages**: Market contracts use the [exponentially-weighted moving average (EMA) price information](https://docs.pyth.network/price-feeds/how-pyth-works/ema-price-aggregation).
- **Maximum Age Limits**: Markets reject stale price data using a configurable expiration duration.

## Oracle Failure Scenarios

### Temporary Outage

- The vast majority of operations will cease to function until fresh price data are available.
- Users can still withdraw collateral from positions with zero liability.
- No borrows or liquidations are supported until fresh price data are available.

### Price Manipulation Attack

- Markets will reject stale prices automatically.
- Defensive asset valuations will protect markets from insolvency in most cases.
- The required maintenance MCR will protect borrowers from unexpected liquidation in most cases.
