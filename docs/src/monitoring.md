# Monitoring and Risk Management

Templar Protocol monitors every deployed contract in real time and publishes its risk data openly. This page describes the monitoring and alerting stack, the public risk dashboard, the operational bots, how to run protocol health checks yourself, and how risk is managed and contained.

## Real-Time Monitoring and Alerting

Templar runs two complementary monitoring systems with 24/7 human coverage:

- **[Hypernative](https://www.hypernative.io/)** monitors the Stellar curated vaults for exploit detection, invariant violations, anomalous privileged calls, and counterparty risk signals.
- **Custom alerts** built by Templar monitor the NEAR markets and proxy oracles. Alert definitions are version-controlled in the [templar-monitoring](https://github.com/Templar-Protocol/templar-monitoring) repository and changed only through reviewed pull requests.

Alerts from both systems are published to the public **[Templar alerts Telegram channel](https://t.me/+CcqXyt01lsljZmQx)**, so integrators and liquidity providers see the same signals the team does.

### What Is Monitored

| Category | Alerts |
|---|---|
| Oracles | Divergence between oracle sources; oracle downtime or stale feeds; circuit breaker trips, re-arms, and configuration changes |
| Positions | Large ("whale") positions; positions approaching liquidation; liquidations executed |
| Solvency | Bad-debt creation; utilization crossing critical thresholds; withdrawal-queue pressure |
| Governance | Proposals created, executed, or cancelled on proxy oracle and vault governance; registry version and deployment events; multisig activity |
| Security | Exploit-pattern detection and invariant checks on vaults (Hypernative) |
| Compliance | Sanctions and risk signals from [Predicate](https://predicate.io/) and [TRM Labs](https://www.trmlabs.com/); SEVERE-labelled accounts interacting with Templar contracts |

### Coverage and Escalation

The three co-founders are distributed across US, EU, and Asia time zones so that a responder is always within working hours. Every alert is classified by severity; critical alerts page all three founders and open a war room following the [emergency runbooks](https://github.com/Templar-Protocol/blend-contracts-v2/tree/main/docs/emergency-runbooks). See [Incident Response](#incident-response) below.

## Live Risk Dashboard

**[data.templarfi.org](https://data.templarfi.org/)** is Templar's public, real-time risk dashboard. It is built from on-chain data and shows, per market and in aggregate:

- **Coverage**: collateral value against outstanding debt, collateralization distribution, and how far positions sit from their maintenance and liquidation thresholds.
- **Liquidation risk**: the price moves that would bring positions into liquidation range and the liquidity available to absorb them.
- Supply, borrow, and utilization levels over time.

The dashboard is the team's own risk-modeling tool as well as a public artifact: the same views drive the position and utilization alerts above. Total value locked is also tracked independently on [DefiLlama](https://defillama.com/protocol/templar-protocol).

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

Markets read prices from the oracle account in their configuration, which for current markets is a [proxy oracle](./oracles.md#proxy-oracle). Check the proxy first, then the underlying adapters.

```bash
# Latest cached price for a feed and its status (accepted, blocked, stale, failed)
near contract call-function as-read-only proxy-oracle-<market>.v1.tmplr.near get_cached_proxy_price json-args '{"id": "<price-id>"}' network-config mainnet now

# Circuit breaker state and accepted price history for a feed
near contract call-function as-read-only proxy-oracle-<market>.v1.tmplr.near get_proxy_circuit_breaker_set json-args '{"id": "<price-id>"}' network-config mainnet now

# What the market will actually see: accepted prices no older than <age> seconds
near contract call-function as-read-only proxy-oracle-<market>.v1.tmplr.near list_ema_prices_no_older_than json-args '{"price_ids": ["<price-id>"], "age": 60}' network-config mainnet now

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

- **Current Interest Rate**: Monitor the current yield rate for supply positions
  ```bash
  near contract call-function as-read-only <market-address> get_last_yield_rate json-args {} network-config mainnet now
  ```
  *Note: Historical interest rate analysis requires an indexer for time-series data; the [risk dashboard](https://data.templarfi.org/) provides this.*

### Network Dependencies

- **NEAR Network**: [NEAR Status](https://status.near.org/)
- **NEAR Intents** (cross-chain deposits and withdrawals): [NEAR Intents Status](https://status.near-intents.org/)
- **Pyth Network**: [Pyth Network Status](https://status.pyth.network/)

## Risk Management

### Economic Risk Assessment

- **Market parameters**: every market's collateralization ratios, interest rate curve, usage ratio, and liquidation spread are immutable and readable via `get_configuration`. The specifications they were deployed from are version-controlled under [`deployments/`](https://github.com/Templar-Protocol/contracts/tree/dev/deployments).
- **Real-time risk modeling**: coverage and liquidation-risk analysis for every market is continuous on the [risk dashboard](https://data.templarfi.org/), rather than a periodic simulation.
- **Oracle reliability**: divergence and downtime are tracked per source and per proxy feed; see [Oracle Health](#oracle-health).
- **Liquidation efficiency**: executed liquidations and positions at risk are alerted in real time and visible on the dashboard.
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
2. **Containment ladder**: halt Templar's own bots; trip the affected proxy oracle feed (freezing borrows, collateral withdrawals against debt, and liquidations on immutable markets); on vaults, Sentinel pause or restriction tightening, allocator abort or rebalance, then curator cap-to-zero or market removal.
3. **User exits are preserved**: no role can disable supply withdrawal requests or repayments.
4. **Recovery** uses a patched market deployed through the registry and voluntary migration, never an in-place upgrade.
5. **Stand-down** requires sign-off from at least two roles, and user-affecting incidents produce a post-mortem.

For the protocol's overall security posture, see [Security](./security-overview.md).
