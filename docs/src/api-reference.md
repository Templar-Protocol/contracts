# API Reference

This guide describes the protocol in prose. The generated Rust API documentation, built from the contract sources with `cargo doc`, is published alongside it and is the authoritative reference for every type, method, argument, and error.

## Rust API Documentation

The API docs are served under [`/doc/`](/doc/templar_common/index.html) on this site. Useful entry points:

| Crate | What it documents | Entry point |
|---|---|---|
| `templar_common` | Types shared by every contract: market configuration and interface, asset and amount types, fees, interest-rate strategies, registry records | [`templar_common`](/doc/templar_common/index.html) |
| Market configuration | The immutable per-market parameters listed on [Risk Parameters](./risk-parameters.md) | [`MarketConfiguration`](/doc/templar_common/market/struct.MarketConfiguration.html) |
| Market interface | Every method a market exposes, with argument and return types | [`MarketExternalInterface`](/doc/templar_common/market/trait.MarketExternalInterface.html) |
| Interest-rate strategies | The `Linear`, `Piecewise`, and `Exponential2` curves | [`InterestRateStrategy`](/doc/templar_common/interest_rate_strategy/enum.InterestRateStrategy.html) |
| Registry records | The `Deployment` record (version key, code hash, block height) returned by `get_deployment` | [`Deployment`](/doc/templar_common/registry/struct.Deployment.html) |
| `templar_proxy_oracle_kernel` | The chain-agnostic proxy oracle pipeline: sources, freshness filters, aggregation, circuit breakers | [`templar_proxy_oracle_kernel`](/doc/templar_proxy_oracle_kernel/index.html) |
| `templar_vault_kernel` | The chain-agnostic vault state machine, fee math, and withdrawal queue | [`templar_vault_kernel`](/doc/templar_vault_kernel/index.html) |
| Contract crates | The NEAR contract entry points themselves | [`templar_market_contract`](/doc/templar_market_contract/index.html), [`templar_registry_contract`](/doc/templar_registry_contract/index.html), [`templar_proxy_oracle_near_contract`](/doc/templar_proxy_oracle_near_contract/index.html) |

The `/doc/` links resolve on the published site ([docs.templarfi.org](https://docs.templarfi.org/)). When serving this guide locally with `mdbook serve` they return 404, because the Rust documentation is built separately by `script/build-docs.sh` and copied next to the guide.

## Gateway JSON-RPC

Integrators who do not want to sign NEAR transactions directly can use the gateway service, a JSON-RPC API in front of the contracts. Its method catalog, generated from the service's own method registry, is at [`gateway/METHODS.md`](https://github.com/Templar-Protocol/contracts/blob/dev/gateway/METHODS.md).

## Calling Contracts Directly

Examples throughout this guide use [`near-cli-rs`](./notes.md#contract-interaction-syntax) view calls. The pages that catalogue them:

- [Market](./contract/market/index.md): configuration, positions, snapshots, and the supply, borrow, and liquidate flows.
- [Registry](./contract/registry.md): versions and deployments.
- [Oracles](./oracles.md#inspecting-a-proxy-oracle): proxy oracle feeds, cached prices, and circuit-breaker state.
- [Monitoring and Risk Management](./monitoring.md#protocol-health-checks): the health checks Templar's own monitoring runs.

**Input needed**: a link to this guide from [templarfi.org](https://templarfi.org/) and from the app, both flagged in the DeFiSafety review, are changes to the website and frontend repositories.
