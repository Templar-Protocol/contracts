# Proxy Oracle Architecture

The proxy oracle keeps a stable market-facing oracle interface while letting each feed use one or more underlying sources.

## Why Proxy Oracles Exist

Markets still point to one oracle account and a pair of price identifiers. Behind that interface, each proxied feed can be composed from multiple sources.

This supports:

- source redundancy without redeploying markets
- source diversity across oracle providers
- transformed feeds for assets whose price depends on another on-chain input
- asset-specific aggregation and freshness policies

## High-Level Architecture

Request flow:

1. The market requests a price from the proxy oracle.
2. The proxy resolves the market-facing price identifier into one or more sources.
3. The proxy fetches source data from contracts such as Pyth and RedStone adapters.
4. If needed, the proxy makes an auxiliary contract call for a transformer input (e.g. LST price derivation).
5. The proxy filters invalid inputs, applies the configured aggregation policy, and returns a single aggregated price.

Each proxied `PriceIdentifier` maps to a feed definition with:

- a set of source entries
- an aggregation method
- freshness and clock-drift filters
- a minimum source threshold

## Feed Construction

A proxy feed's source entries may be:

- direct Pyth requests
- direct RedStone requests
- transformer-based requests that combine an oracle price with an on-chain input

This allows single-source, primary-plus-backup, multi-source, and derived feeds.

## Key Properties

### Stable Market Interface

Markets do not need failover logic. They still read one oracle account and one set of price identifiers.

### Per-Feed Aggregation

Each feed has its own aggregation policy:

- `MedianLow`: conservative median across sources
- `Priority`: highest-weight source wins

This avoids a single global policy for all assets.

### Confidence-Aware Aggregation

The proxy aggregates over source confidence bounds, not just point estimates. This is more conservative than a simple passthrough or first-response model.

### Layered Freshness Controls

Freshness is checked at multiple layers:

- the market asks for prices no older than its configured maximum age
- the proxy filters stale or future-drifted prices again at the feed level
- the proxy can require a minimum number of live sources before returning a price

### Native Support For Transformed Assets

Transformers are useful for assets such as LSTs. If the LST has no direct oracle price, but the underlying asset price is available from the oracle and the redemption rate is available on-chain, the proxy can derive the LST price from those inputs.

## How Backup Oracles Work

Proxy oracle definitions can be configured to be resilient against [price manipulation attacks](https://rekt.news/yieldblox-rekt).

Instead of changing the market's oracle contract during an incident, the proxied feed itself can be updated. A feed can define:

- one source acts as the preferred source
- one or more additional sources act as backups
- the aggregator determines how multiple live sources are combined

Flow:

1. A market-facing price identifier points to one proxy definition.
2. That definition contains primary and backup sources.
3. The proxy applies its aggregation rule to the live valid sources.
4. The market keeps the same oracle configuration.

Benefits:

- markets keep the same oracle endpoint and identifier
- backup activation can be handled as a feed-definition change instead of a market migration

## Governance Model

Governance actions may only be executed by the proxy oracle contract's owner.

Proxy oracle governance is simple:

- feed definitions are controlled through an on-chain proposal process
- proposed changes are delayed by a configurable TTL
- proposals can be inspected before execution
- proposals can be executed or cancelled after review

The TTL starts at zero on deployment. Operators should set a non-zero TTL during initial setup.

Supported actions:

- set, update, or remove a proxied feed definition
- change the TTL used for future governance actions

Feed changes are not hidden off-chain configuration. They go through an explicit on-chain workflow.

## Governance Workflow

Workflow:

1. Create a proposal.
2. Wait for the TTL.
3. Review the live proposal.
4. Execute or cancel it.

## Risk Controls

Key mitigations:

- multiple independent source options per feed
- explicit freshness thresholds
- minimum-source requirements
- conservative aggregation logic
- transparent on-chain governance for feed updates

Remaining risks:

- correlated failures across upstream sources
- bad configuration of feed definitions or weights
- liveness issues if too few sources remain fresh
- operational dependency on the governance process for feed changes
- no inter-source deviation check; a bad source can still influence aggregation
- no absolute price sanity bounds
- governance can set the TTL back to zero, removing the delay for future proposals
- there is no emergency shortcut around the active TTL

## Transparency

Key transparency properties:

- markets continue using a simple, inspectable oracle interface
- proxied feed definitions are queryable on-chain
- governance proposals are queryable on-chain
- backup-source support is built into production oracle infrastructure

For operator details, governance procedures, and tooling workflows, see the [Proxy Oracle Operations Runbook](./runbook.md).
