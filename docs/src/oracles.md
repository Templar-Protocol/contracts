# Oracles

Templar Protocol relies on external price oracles to determine asset valuations when calculating collateralization ratios and performing liquidations. Every market operation that needs a price (borrowing, withdrawing collateral from a position with outstanding debt, and liquidating) reads the market's configured oracle account and **fails closed** if no acceptably fresh price is available.

## Oracle Providers

| Provider | Status | How Templar uses it |
|---|---|---|
| [Pyth Network](https://pyth.network/) | Live | Primary price source. Newer markets read [Pyth Lazer](https://docs.pyth.network/lazer) (marketed by Pyth as Pyth Pro) through Templar's Lazer adapter contract; markets deployed before the proxy oracle read Pyth's on-chain pull oracle ([`pyth-oracle.near`](https://nearblocks.io/address/pyth-oracle.near)) directly. |
| [RedStone](https://redstone.finance/) | Live | Independent second source through Templar's RedStone adapter contract. RedStone is also the sole source for some tokenized real-world-asset feeds (for example tokenized treasury and fund products) that Pyth does not yet publish. |
| [Chainlink](https://chain.link/) | Planned | Will be added as an additional proxy oracle source. |
| Atlas | Planned | Will be added as an additional proxy oracle source. |

Where a market reads more than one provider, the sources are combined by a [proxy oracle](#proxy-oracle) rather than by the market itself. Source weights, quorum, freshness bounds, and circuit breakers are configured per market and can be adjusted through the proxy oracle's timelocked governance.

Adding a new provider does not require changes to the market contract: a new adapter is deployed and registered as a source on the relevant proxy oracles.

## How Markets Read Prices

Every market is [configured](/doc/templar_common/market/struct.PriceOracleConfiguration.html) with a single oracle account and a price identifier for each of its two assets:

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

The market calls `list_ema_prices_no_older_than` on that account, the same read interface exposed by Pyth's NEAR contract. Templar's proxy oracle implements the same interface, so a market can read a proxy oracle, the Pyth contract, or the LST adapter without any change to the market code. Which one a market uses is visible in its `get_configuration` output.

Because this configuration is immutable, an existing market cannot be repointed to a different oracle account by an administrator. Markets deployed before the proxy oracle existed were migrated to proxy oracles through a reviewed, dry-run-verified storage patch (see [Deploying a market](./deployments.md#patching-contract-storage)); new markets are deployed reading a proxy oracle from the start.

## Proxy Oracle

The proxy oracle is Templar's oracle aggregation and safety layer. Each market typically has its own proxy oracle and governance contract, deployed alongside the market through the registry.

- Source: [`contract/proxy-oracle`](https://github.com/Templar-Protocol/contracts/tree/dev/contract/proxy-oracle)
- Audit: [Halborn, proxy oracle security assessment (August 2026)](https://drive.google.com/file/d/1KOVYEiz8pGcWRWJ_-NFDa94LJ_f2zMWw/view?usp=sharing). All findings were remediated before deployment; the remediation is recorded in the repository history.

The logic is split between a chain-agnostic kernel (aggregation, freshness, circuit breakers) and per-chain runtimes. The NEAR runtime serves Templar markets; a Soroban runtime with the same kernel serves Stellar consumers through SEP-40 adapters.

### Price Pipeline

For each proxied price identifier, an update runs through the following stages:

1. **Sources.** The proxy fetches each configured source asynchronously. A source is a Pyth Lazer feed, a classic Pyth feed, a RedStone feed, or a *transformer* (for example, a liquid-staking-token normalization that multiplies an underlying price by an on-chain redemption rate).
2. **Freshness filter.** Prices older than the configured `max_age` or timestamped further in the future than `max_clock_drift` are discarded before aggregation, so a stale or pre-manufactured price can never be replayed into the aggregate.
3. **Aggregation.** Surviving prices are combined by the proxy's aggregator. The production configuration uses a **weighted median**: `median_low` for collateral assets and `median_high` for borrow assets, so that any tie or ambiguity resolves to the valuation that is safer for the protocol. A `min_sources` quorum is enforced; if fewer fresh sources than the quorum survive, the update fails and nothing is cached. A priority aggregator (first fresh source wins) is also available.
4. **Circuit breakers.** The aggregated candidate is checked against the feed's [circuit breaker set](#circuit-breakers). An enforced breaker that trips blocks the feed; a breaker running in observe-only mode records the trip and emits an event without blocking.
5. **Cache.** The accepted price is cached with its status. Markets read only accepted, fresh cached prices; a missing, blocked, failed, or stale cache entry reads as *no price*, and the market operation fails closed.

Standard mainnet feeds are configured with a Pyth Lazer source at weight 8 and a RedStone source at weight 2 with `min_sources = 1`. With that weighting the Pyth price determines the aggregate while both sources are fresh, and RedStone keeps the feed alive if Pyth is stale or unavailable. Because the quorum is one, a single fresh source can determine the price when the other is stale, and the higher-weighted source determines it while both are fresh: this quorum provides liveness, not manipulation resistance. For a feed configured this way, the freshness filter and the enforced circuit breakers are the defence against a compromised provider. Weights, quorum, and sources are governed per feed and can be raised (for example to a quorum of two once a third provider is live) as additional providers come online.

### Circuit Breakers

Circuit breakers are the on-chain defence against oracle manipulation and runaway prices. They evaluate each candidate price against the feed's *accepted* history (not against the previous raw sample, which an attacker may already have manipulated). Four rule types are available and can be combined:

| Rule | Trips when |
|---|---|
| `StepwiseChange` | A single update moves more than `max_relative_change` from the last accepted price. Catches sudden jumps. |
| `MonotonicRun` | At least `max_streak` consecutive accepted updates move in the same direction by at least `min_relative_step_change` each. Catches staged ramps. |
| `WindowedChangeDelta` | The mean of the most recent `window_len` observations differs from the mean of an earlier window by more than `max_relative_mean_change`. |
| `CumulativeChange` | The price drifts more than `max_relative_change` from an immutable baseline captured when the rule was installed. |

In addition to automatic rules, an operator holding the `ManualTripper` role can **trip a feed manually with no timelock**. Because market contracts have no pause function, the manual trip is the emergency brake for an immutable market: while a feed is blocked, borrowing, collateral withdrawal against debt, and liquidation on every market reading that feed stop, while supply withdrawals, repayments, and collateral withdrawal from debt-free positions continue.

Each feed keeps up to 32 accepted observations of history and up to 16 breakers. Breakers can be installed in observe-only mode (they trip and emit events but do not block) before being enforced. Re-arming a tripped breaker is a separate governed action, so a block cannot be lifted by the same automation that detected the problem.

Every configuration change, trip, re-arm, and enforcement change emits an on-chain event, which is what Templar's [monitoring](./monitoring.md) watches.

### Governance and Timelocks

Each proxy oracle is owned by its own governance contract. Feed configuration (sources, weights, aggregator, freshness filter, circuit breaker rules) and contract upgrades change only through proposals that mature under a per-method timelock and can only be executed by an account holding the role that method requires. Circuit breaker *operation* is different: tripping and untripping a feed, re-arming a breaker, and switching enforcement on or off are role-gated actions with no delay.

Timelocks and roles are configured per governance contract. The table below is the typical production setup; it is not guaranteed to apply to every proxy oracle, so confirm the policy in force on a specific contract as shown at the end of this section.

| Action | Timelock | Required role |
|---|---|---|
| Trip or untrip a feed manually | none (immediate) | `ManualTripper` |
| Re-arm a tripped breaker; enable or disable enforcement | none (immediate) | `CircuitBreakerOperator` |
| Set or change a feed's sources, weights, aggregator, or freshness filter | 24 hours | `ProxyConfigurationManager` |
| Add, remove, or reconfigure circuit breakers | 24 hours | `ProxyConfigurationManager` |
| Any other proxy oracle method (conservative default) | 72 hours | `Admin` |
| Upgrade the proxy oracle contract code | 72 hours | `Admin` |
| Change the governance policy (timelocks, roles) | 48 hours | `Admin` |
| Upgrade the governance contract itself | 168 hours | `Admin` |

The immediate actions include their risk-increasing inverses: a `ManualTripper` can untrip a feed and a `CircuitBreakerOperator` can disable enforcement without delay. Those roles are therefore held by the same multisig as `Admin`, every use emits an on-chain event that monitoring alerts on, and a change to a breaker's *rules* still waits out the configuration timelock. A proposal's timelock is fixed when it is created, and shortening any timelock must itself mature under the timelock being shortened, so governance cannot be weakened faster than it currently protects. The `Admin` role is held by Templar's 2-of-3 multisig; see [Protocol Governance](./governance.md).

The policy in force on a specific proxy oracle can be read from its governance contract:

```bash
near contract call-function as-read-only \
    proxy-gov-<market>.v1.tmplr.near get_governance_policy \
    json-args '{}' network-config mainnet now
```

and pending proposals with `list_proposals` and `get_proposal`.

### Updating Proxy Prices

The proxy oracle is a **pull** design: someone must refresh it before a price-dependent action.

1. Push fresh data to the underlying adapters (a signed Pyth Lazer payload to the Lazer adapter, a signed RedStone data package to the RedStone adapter, or a Pyth proof to `pyth-oracle.near`).
2. Call `update_prices` on the proxy oracle with the price identifiers the market uses. The proxy fetches the sources, runs the pipeline above, and caches the result.
3. Perform the market action.

Templar's relayer and liquidation bots do this automatically ahead of the actions they submit, and the Templar gateway exposes `oracle.updatePrices` to perform every required update for a market in one call. Integrators submitting their own transactions must do the same. For a market configured with a proxy oracle, reading the underlying Pyth or RedStone contracts directly bypasses the aggregation and circuit-breaker guarantees and is not the price the market will use; for a legacy market that reads `pyth-oracle.near` or the LST adapter directly, that configured contract is the market price.

### Inspecting a Proxy Oracle

```bash
# Which price identifiers this proxy serves
near contract call-function as-read-only \
    proxy-oracle-<market>.v1.tmplr.near list_proxies \
    json-args '{}' network-config mainnet now

# Sources, weights, aggregator, and freshness filter for one feed
near contract call-function as-read-only \
    proxy-oracle-<market>.v1.tmplr.near get_proxy \
    json-args '{"id":"<price-id>"}' network-config mainnet now

# Circuit breakers, their state, and accepted history for one feed
near contract call-function as-read-only \
    proxy-oracle-<market>.v1.tmplr.near get_proxy_circuit_breaker_set \
    json-args '{"id":"<price-id>"}' network-config mainnet now

# Latest cached result and its status (accepted, blocked, stale, failed)
near contract call-function as-read-only \
    proxy-oracle-<market>.v1.tmplr.near get_cached_proxy_price \
    json-args '{"id":"<price-id>"}' network-config mainnet now
```

## Oracle Addresses

### Mainnet

| Contract | Account ID | Purpose |
|---|---|---|
| Proxy oracle (one per market) | `proxy-oracle-<market>.v1.tmplr.near` | Aggregated, circuit-breaker-protected feed read by the market `<market>.v1.tmplr.near`. Example: [`proxy-oracle-ixlmdejaaa-ixlmusdc-2.v1.tmplr.near`](https://nearblocks.io/address/proxy-oracle-ixlmdejaaa-ixlmusdc-2.v1.tmplr.near). |
| Proxy oracle governance (one per proxy) | `proxy-gov-<market>.v1.tmplr.near` | Timelocked governance for the matching proxy oracle. Example: [`proxy-gov-ixlmdejaaa-ixlmusdc-2.v1.tmplr.near`](https://nearblocks.io/address/proxy-gov-ixlmdejaaa-ixlmusdc-2.v1.tmplr.near). |
| Pyth Lazer adapter | [`pyth-lazer.v1.tmplr.near`](https://nearblocks.io/address/pyth-lazer.v1.tmplr.near) | Verifies signed Pyth Lazer payloads (ed25519, trusted signer set, freshness window, per-feed anti-replay) and serves them by Lazer feed ID. Updates are permissionless because authenticity is cryptographic. |
| RedStone adapter | [`redstone-adapter.v1.tmplr.near`](https://nearblocks.io/address/redstone-adapter.v1.tmplr.near) | Verifies RedStone signed data packages against the configured signer threshold and serves them by feed ID. |
| Pyth (classic pull oracle) | [`pyth-oracle.near`](https://nearblocks.io/address/pyth-oracle.near) | Pyth's own contract. Read directly by markets deployed before the proxy oracle, and usable as a proxy source. |
| LST oracle adapter | [`lst.oracle.tmplr.near`](https://nearblocks.io/address/lst.oracle.tmplr.near) | Legacy adapter deriving liquid-staking-token prices; see below. |

The exact oracle account and price identifiers a market uses are always available from the market's `get_configuration` view; see [Smart Contract Addresses](./addresses.md) for the market list. A market can also read a proxy oracle that was deployed separately from it (for example `ixlmustry-ixlmusdc.v1.tmplr.near` reads `proxy-oracle-ixlmustry-ixlmusdc.v1.tmplr.near`, deployed on its own), so the governance account is not always `proxy-gov-<market>`; the authoritative link is the proxy oracle's owner, returned by its `own_get_owner` view.

### Testnet

| Contract | Account ID |
|---|---|
| Pyth (classic pull oracle) | [`pyth-oracle.testnet`](https://testnet.nearblocks.io/address/pyth-oracle.testnet) |

## Price Identifiers

- **Proxy oracle feeds** use a 32-byte identifier chosen when the feed is configured. Use `list_proxies` on the proxy oracle to enumerate them, and `get_proxy` to see which underlying feeds each one aggregates.
- **Pyth** price identifiers can be found on [Pyth's documentation site](https://docs.pyth.network/price-feeds/price-feeds#feed-ids). Pyth Lazer feeds are addressed by their numeric Lazer feed ID.
- **RedStone** feeds are addressed by their feed ID (for example `BTC`, `USDC`, or a fundamental-value feed such as `deJAAA_FUNDAMENTAL/USD`).

## LST Oracle Adapter

For Liquid Staking Tokens (LSTs), Templar uses [a custom oracle adapter](./contract/lst-oracle.md) ([`lst.oracle.tmplr.near`](https://nearblocks.io/address/lst.oracle.tmplr.near)) to derive the LST price from the underlying asset price and the staking contract's redemption rate. The same normalization is available inside the proxy oracle as a *transformer* source, which is the path new LST markets use.

## Update Frequency and Freshness

- **Update model**: Pull. Prices are pushed on-chain as needed by relayers, bots, and users, and the proxy oracle is refreshed before price-dependent operations.
- **Source freshness**: Enforced per feed by the proxy oracle's freshness filter before aggregation.
- **Market staleness bound**: Each market rejects prices older than its configured `price_maximum_age_s`, typically 60 to 120 seconds.
- **Confidence and smoothing**: Markets consume Pyth-style prices with a confidence interval and use exponentially-weighted moving average (EMA) prices where the source provides them.

## Price Validation

Markets validate price freshness before use. If no fresh, accepted price is available, operations that require prices (borrow, collateral withdrawal against debt, liquidate) fail, and callers must refresh the oracle first. Supply deposits, supply withdrawals, repayments, and collateral withdrawals from positions with no liability do not depend on the oracle.

## Oracle Security Measures

- **Multiple independent sources**: Pyth and RedStone are aggregated per feed with a configurable quorum. With the standard quorum of one this guarantees liveness through a single-provider outage; raising the quorum trades liveness for manipulation resistance.
- **Freshness filters**: Stale and future-dated source prices are discarded before aggregation.
- **Conservative aggregation**: Weighted median, biased low for collateral and high for liabilities.
- **Confidence intervals**: Pyth prices include confidence bands. The lower bound is used for collateral valuations and the upper bound for liability valuations.
- **Circuit breakers**: Automatic rules against jumps, ramps, and drift, plus an instant manual trip. See [Circuit Breakers](#circuit-breakers).
- **Timelocked governance**: No feed configuration can change without a maturing proposal; only risk-reducing actions are immediate.
- **Maximum age limits**: Markets reject stale price data using a configurable expiration duration.
- **Fail-closed reads**: A blocked, failed, or stale feed reads as no price, never as the last known price.
- **Monitoring**: Oracle divergence and downtime alerts run continuously; see [Monitoring and Risk Management](./monitoring.md).

## Oracle Failure Scenarios

### Temporary Outage of a Provider

- If one provider is stale but the feed's quorum is still met, the aggregate continues from the remaining fresh sources.
- If no fresh source meets the quorum, the feed reads as no price and price-dependent operations pause until data returns.
- Users can still withdraw supply, repay debt, and withdraw collateral from positions with zero liability.

### Circuit Breaker Trip

- Automatic trips and manual trips have the same effect: the affected feed is blocked and every market reading it stops borrowing, collateral withdrawal against debt, and liquidations.
- Positions are not liquidated on a blocked price. When the feed is re-armed, liquidations resume against the newly accepted price.
- Trips are announced on the public alerts channel; see [Monitoring and Risk Management](./monitoring.md).

### Price Manipulation Attack

- An attacker who controls a source must move the weighted median (with the standard weighting the Pyth feed determines it, and a lone fresh source determines it while the other is stale), survive the freshness filter, and pass every enforced breaker in the same update. The breakers compare against accepted history, so a manipulated prior sample does not weaken them.
- Markets reject stale prices automatically, and defensive valuations (lower bound for collateral, upper bound for liabilities) protect solvency in most cases.
- The required maintenance collateralization ratio protects borrowers from unexpected liquidation in most cases.

## Roadmap

Two changes to the oracle layer are planned; neither has a committed date (**Input needed**: timing).

- **Raise the per-feed quorum once a third provider is live.** With Pyth and RedStone as the only sources, `min_sources = 1` is what keeps a feed live through a single-provider outage. When a third independent provider (Chainlink or Atlas, both listed as planned under [Oracle Providers](#oracle-providers)) is configured on a feed, the quorum can be raised to two so that no single provider determines the price on its own. The change is a timelocked configuration proposal on each proxy oracle's governance contract.
- **Retire classic Pyth reads in favour of Pyth Lazer.** Several proxy oracles still read `pyth-oracle.near`; the shared asset profiles in [`deployments/profiles/`](https://github.com/Templar-Protocol/contracts/tree/dev/deployments/profiles) already describe the Lazer configuration each feed migrates to. Each migration is a timelocked configuration proposal.
