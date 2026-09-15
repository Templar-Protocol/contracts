# Monitoring and Risk Management

Templar Protocol runs real-time monitoring on its NEAR markets and proxy oracles and on its Stellar curated vaults, and publishes its risk data openly. This page describes the monitoring and alerting stack, the public risk dashboard, the operational bots, how to run protocol health checks yourself, and how risk is managed and contained.

## Real-Time Monitoring and Alerting

Templar runs two complementary monitoring systems with 24/7 human coverage:

- **[Hypernative](https://www.hypernative.io/)** monitors the Stellar curated vaults for exploit detection, invariant violations, anomalous privileged calls, and counterparty risk signals.
- **Custom alerts** built by Templar monitor the NEAR markets and proxy oracles. Alert definitions are version-controlled in the [templar-monitoring](https://github.com/Templar-Protocol/templar-monitoring) repository and changed only through reviewed pull requests.

Alerts from both systems are published to the public **[Templar alerts Telegram channel](https://t.me/+CcqXyt01lsljZmQx)**, so integrators and liquidity providers see the same signals the team does.

### What Is Monitored

The alerting covers:

- Oracle failure or price deviation beyond threshold
- Position approaching liquidation
- Liquidation executed
- Large position events (whale alerts)
- Bad debt creation
- Utilization rate crossing critical thresholds
- Smart contract pause or emergency admin action

### Coverage and Escalation

Templar's security policy defines an on-call rotation among the multisig signers, who are distributed across multiple time zones, so that a responder is within working hours at all times. Every alert is classified by severity; critical alerts page all signers and open a war room following the [emergency runbooks](https://github.com/Templar-Protocol/blend-contracts-v2/tree/main/docs/emergency-runbooks). See [Incident Response](#incident-response) below.

### Compliance Screening

Separately from the protocol alerts above, Templar screens on-chain activity against sanctions and risk signals from [Predicate](https://predicate.io/) and [TRM Labs](https://www.trmlabs.com/). SEVERE-labelled accounts interacting with Templar contracts trigger a Telegram notification. Details are in the [templar-monitoring](https://github.com/Templar-Protocol/templar-monitoring) repository.

## Live Risk Dashboard

**[data.templarfi.org](https://data.templarfi.org/)** is Templar's public, real-time risk dashboard. It is built directly from on-chain data (contract state over NEAR RPC, blocks and events from NEARData and FastNear) with USD prices from Pyth, Pyth Lazer, and RedStone, and it is the same tool the team uses for risk modeling. Every view can be filtered per market and viewed in aggregate.

**Ops Console**

- **[Collateral Coverage](https://data.templarfi.org/#coverage)**: total collateral against outstanding borrows, a 90-day coverage trend, coverage by collateral asset, and a per-asset breakdown with each market classified as healthy, below target, or undercollateralized. Markets whose price feed was stale or missing at scan time are flagged rather than silently counted.
- **[Liquidation Proximity](https://data.templarfi.org/#liquidation)** (live): positions at risk (health factor at or below 1.2), aggregate coverage ratio, average health factor, the health-factor distribution ("collateral wall"), and scenario sliders showing how a price drawdown moves positions toward liquidation.
- **[Oracle Health](https://data.templarfi.org/#oracle)**: assets scanned, stale feeds, and missing feeds; per-feed staleness, spread, and confidence; effective (protocol-side) versus raw price; and a per-asset oracle state table.
- **[TVL & Revenue](https://data.templarfi.org/#tvl)**: total supply, total borrowed, and protocol TVL across day, week, month, and quarter; protocol fees and cumulative revenue.

**Risk Committee**

- **[Utilization & Rates](https://data.templarfi.org/#utilization)**: current utilization against each market's kink, the interest rate curves, and 90-day history.
- **[Concentration Risk](https://data.templarfi.org/#concentration)**: top-1 and top-5 borrower share, Herfindahl-Hirschman indices for borrowers, collateral providers, and suppliers, single-borrower and single-provider exposure, and the distributions and top lists behind them.

**Strategic**

- **[Supply-Side Analytics](https://data.templarfi.org/#supply)**: deposit and withdrawal flows, net supply flow, depositor count, average deposit age, accrued yield, and advertised supply APY; borrow and repay flows and net loan flow, with a market selector.
- **[Positions at Risk](https://data.templarfi.org/#stress)**: position counts by risk tier (liquidatable below 1.0 health factor, critical below 1.10, warning below 1.25, healthy) with an hourly trend.
- **[Ledger Events](https://data.templarfi.org/#events)**: deposits, withdrawals, borrows, repays, and liquidations, newest first, filterable by market.
- **[Intents Transfers](https://data.templarfi.org/#transfers)**: where value went after withdrawal, showing bridge-outs and NEAR transfers by account through the NEAR Intents settlement layer, which block explorers cannot show.

The same data drives the position and utilization alerts above. Total value locked is also tracked independently on [DefiLlama](https://defillama.com/protocol/templar-protocol).

## Operational Services

The protocol runs the following off-chain services, all open source under [`service/`](https://github.com/Templar-Protocol/contracts/tree/dev/service):

- **Liquidator**: monitors borrow positions across all registered markets, refreshes oracle prices, and executes liquidations of under-collateralized positions. Liquidation is permissionless; Templar's bot is one participant, not a privileged one.
- **Accumulator**: applies interest to borrow positions on a schedule so that accrued liability is always reflected on-chain.
- **Market monitor**: scans every market and sends Telegram alerts for positions at risk of liquidation, classified into health zones by distance from the maintenance ratio.
- **Oracle updaters**: the RedStone bridge service, together with the oracle-update paths built into the liquidator and relayer, fetch signed Pyth Lazer and RedStone payloads, submit them to the on-chain adapters, and refresh proxy oracle prices ahead of price-dependent actions.
- **Relayer**: relays signed delegate actions so that accounts without NEAR can interact with the protocol, and refreshes oracle prices before the actions it relays.

Bots can be halted at any time as a containment step. They have no privileged access to markets: everything they do, anyone can do.

### Gas Usage Monitoring

Gas analysis tools provide performance insights:

```bash
./script/gas-report.sh
```

This generates detailed reports on function execution costs, snapshot iteration limits, and performance bottlenecks.

## Protocol Health Checks

Anyone can reproduce the core health checks with view calls. Examples use [`near-cli-rs`](./notes.md#contract-interaction-syntax).

### Market Status

```bash
# Get market configuration (immutable after deployment)
near contract call-function as-read-only <market-address> get_configuration json-args {} network-config mainnet now

# Check current market snapshot
near contract call-function as-read-only <market-address> get_current_snapshot json-args {} network-config mainnet now

# Get borrow asset metrics (supplied, borrowed, available)
near contract call-function as-read-only <market-address> get_borrow_asset_metrics json-args {} network-config mainnet now

# List all deployed markets from the registry
near contract call-function as-read-only v1.tmplr.near list_deployments json-args '{"offset": 0, "count": 100}' network-config mainnet now
```

`paid_to_fees` is the decimal-string amount of repaid interest and fees
currently available to pay supplier yield. It is a market-wide,
first-come-first-served pool: it is not accrued yield, per-user reserved
cash, or a guarantee that a withdrawal will execute. A raw JSON response
that omits this field is from a legacy deployment and differs from an
upgraded market explicitly reporting `"0"`; Rust consumers deserialize an
omitted field as zero for compatibility and therefore lose that distinction.
The field appears on live markets only after a released version containing it
has been deployed or upgraded.

### Oracle Health

Markets read prices from the oracle account in their configuration. Newer markets read a [proxy oracle](./oracles.md#proxy-oracle); some older markets still read `pyth-oracle.near` or `lst.oracle.tmplr.near` directly (for example `ibtc-iethusdc.v1.tmplr.near` and `stnear-usdc-1.v1.tmplr.near`), so check the market's `get_configuration` output first. For a proxy-backed market, check the proxy first, then the underlying adapters. The example below uses `ixlmdejaaa-ixlmusdc-2.v1.tmplr.near`, whose configuration points to `proxy-oracle-ixlmdejaaa-ixlmusdc-2.v1.tmplr.near`; take the price identifiers from that configuration.

```bash
# Latest cached price for a feed and its status (accepted, blocked, stale, failed)
near contract call-function as-read-only proxy-oracle-ixlmdejaaa-ixlmusdc-2.v1.tmplr.near get_cached_proxy_price json-args '{"id": "<price-id>"}' network-config mainnet now

# Circuit breaker state and accepted price history for a feed
near contract call-function as-read-only proxy-oracle-ixlmdejaaa-ixlmusdc-2.v1.tmplr.near get_proxy_circuit_breaker_set json-args '{"id": "<price-id>"}' network-config mainnet now

# What the market will actually see: accepted prices no older than <age> seconds
near contract call-function as-read-only proxy-oracle-ixlmdejaaa-ixlmusdc-2.v1.tmplr.near list_ema_prices_no_older_than json-args '{"price_ids": ["<price-id>"], "age": 60}' network-config mainnet now

# Underlying sources
near contract call-function as-read-only pyth-oracle.near get_price json-args '{"price_identifier": "<pyth-price-id>"}' network-config mainnet now
near contract call-function as-read-only redstone-adapter.v1.tmplr.near read_price_data_for_feed json-args '{"feed_id": "<feed-id>"}' network-config mainnet now

# Legacy LST oracle adapter
near contract call-function as-read-only lst.oracle.tmplr.near get_price_data json-args '{}' network-config mainnet now
```

Provider status pages: [Pyth Network Price Feeds](https://insights.pyth.network/price-feeds) and [Pyth Network Status](https://status.pyth.network/); [RedStone](https://redstone.finance/).

### Market Data Analysis

- **Supply Positions**: Monitor individual and aggregate supply positions
  ```bash
  near contract call-function as-read-only <market-address> list_supply_positions json-args '{"offset": 0, "count": 100}' network-config mainnet now
  ```

- **Withdrawal Queue**: Check pending withdrawal requests
  ```bash
  near contract call-function as-read-only <market-address> get_supply_withdrawal_queue_status json-args {} network-config mainnet now
  ```

- **Historical Snapshots**: Analyze market history
  ```bash
  near contract call-function as-read-only <market-address> list_finalized_snapshots json-args '{"offset": 0, "count": 10}' network-config mainnet now
  ```

- **Utilization Rate**: Calculate from borrow asset metrics as `borrowed / (borrowed + available)`
  ```bash
  near contract call-function as-read-only <market-address> get_borrow_asset_metrics json-args {} network-config mainnet now
  ```

- **Supplier Yield Availability**: `min(accrued_yield, paid_to_fees)` is the
  largest yield-only request that can be paid without consuming principal. Such
  a request meets the configured minimum only when
  `min(accrued_yield, paid_to_fees) >= supply_withdrawal_range.minimum`. This is
  not a general withdrawal-eligibility check: principal may cover the remainder
  of a larger request. Existing eligibility checks still apply, and other
  withdrawals can consume the shared pool before execution.

- **Last Yield Rate**: `get_last_yield_rate` returns the most recently computed supply yield rate. It is an *expected average over time*, not a spot rate; supply positions earn yield the instant it is distributed.
  ```bash
  near contract call-function as-read-only <market-address> get_last_yield_rate json-args {} network-config mainnet now
  ```
  *Note: Historical interest rate analysis requires an indexer for time-series data; the [risk dashboard](https://data.templarfi.org/) provides this.*

### Network Dependencies

- **NEAR Network**: [NEAR Status](https://status.near.org/)
- **NEAR Intents** (cross-chain deposits and withdrawals): [NEAR Intents Status](https://status.near-intents.org/)
- **Pyth Network**: [Pyth Network Status](https://status.pyth.network/)

Templar's cross-chain collateral and stablecoin flows depend on NEAR infrastructure: NEAR Intents, the Omnibridge, Chain Signatures (the MPC network), and nearcore itself. The security audits of those dependencies (nearcore assessments by Trail of Bits and Sigma Prime; NEAR Intents reviews by Hacken, Guvenkaya, zkSecurity, and Aurora Labs; Omnibridge audit reports; and the NEAR One reports on the MPC Chain Signatures network) are collected in the [NEAR dependency audits folder](https://drive.google.com/drive/folders/1_6MPZLrWxLTWpCi5caYC2IWKP-8sW1uP?usp=sharing). Contingency procedures for a dependency incident are in the [emergency runbooks](https://github.com/Templar-Protocol/blend-contracts-v2/tree/main/docs/emergency-runbooks).

## Risk Management

### Economic Risk Assessment

- **Market parameters**: every market's collateralization ratios, interest rate curve, usage ratio, and liquidation spread are immutable and readable via `get_configuration`. The specifications they were deployed from are version-controlled under [`deployments/`](https://github.com/Templar-Protocol/contracts/tree/dev/deployments).
- **Real-time risk modeling**: coverage, liquidation proximity, positions by risk tier, concentration, and utilization analysis for every market is continuous on the [risk dashboard](https://data.templarfi.org/), rather than a periodic simulation.
- **Oracle reliability**: divergence and downtime are tracked per source and per proxy feed, and the dashboard's Oracle Health view shows staleness, spread, and confidence per feed; see [Oracle Health](#oracle-health).
- **Liquidation efficiency**: executed liquidations and positions at risk are alerted in real time and visible in the dashboard's Liquidation Proximity, Positions at Risk, and Ledger Events views.
- **Individual positions**: [My Account](https://app.templarfi.org/borrow-supply/my-account) in the Templar app.

### Risk Mitigation Strategies

- **Isolated markets**: one collateral and one borrow asset per market, so no cross-asset contagion.
- **Immutable markets**: no parameter can be changed after deployment, by anyone.
- **Conservative parameters**: maintenance ratios above liquidation ratios, bounded liquidation spreads, and usage-ratio caps that preserve withdrawal liquidity.
- **Multi-source oracles with circuit breakers**: see [Oracles](./oracles.md).
- **Defensive valuation**: collateral valued at the lower confidence bound and liabilities at the upper bound.
- **Liquidation incentives**: permissionless, partially-liquidating liquidations with a configured spread so that positions are restored to health promptly.
- **Dynamic interest rates**: utilization-driven curves that price liquidity scarcity and pull utilization back toward the optimum.
- **Curated vaults**: professional curators manage vault allocations under caps, timelocks, and an independent Sentinel; see the [Curator Guide](./curator-guide.md).

## Incident Response

Templar's response to an alert follows the public [emergency runbooks](https://github.com/Templar-Protocol/blend-contracts-v2/tree/main/docs/emergency-runbooks):

1. Whoever spots an alert opens a war room, classifies severity, and takes the smallest reversible mitigation for their role.
2. **Containment ladder**: halt Templar's own bots; trip the affected proxy oracle feed (freezing borrows, collateral withdrawals against debt, and liquidations on markets that read a proxy oracle); on vaults, Sentinel pause or restriction tightening, allocator abort or rebalance, then curator cap-to-zero or market removal.
3. **User exits are preserved**: no role can disable supply withdrawal requests or repayments.
4. **Recovery** uses a patched market deployed through the registry and voluntary migration, never an in-place upgrade.
5. **Stand-down** requires sign-off from at least two roles, and user-affecting incidents produce a post-mortem.

For the protocol's overall security posture, see [Security](./security-overview.md).
