# API Reference

Templar exposes three programmatic surfaces. The **backend HTTP API** is the main reference for applications and integrators; the **gateway JSON-RPC** service and **direct contract calls** are lower-level paths for those who need them. The generated Rust documentation covers the contract types every surface returns.

## Backend HTTP API

The backend HTTP API is the primary way to read protocol data and prepare user flows without handling contract calls yourself: a conventional web API over HTTPS with JSON request and response bodies. Its interactive documentation is the authoritative catalogue of endpoints, parameters, schemas, examples, and error responses:

**[api.templarfi.org/docs](https://api.templarfi.org/docs)**

The live reference is split into eight OpenAPI documents:

| API | Use it for | Live reference |
|---|---|---|
| Markets | Market configuration and metrics, positions, wallet balances, prices, and asset metadata | [Markets](https://api.templarfi.org/docs?spec=markets#tag/markets), [accounts](https://api.templarfi.org/docs?spec=markets#tag/accounts), [prices](https://api.templarfi.org/docs?spec=markets#tag/prices), [assets](https://api.templarfi.org/docs?spec=markets#tag/assets) |
| Lending | Preparing borrow, repay, collateral, and supply transactions | [Borrow](https://api.templarfi.org/docs?spec=lending#tag/borrow), [supply](https://api.templarfi.org/docs?spec=lending#tag/supply) |
| Swaps | Supported tokens, quotes, deposit confirmation, and swap status | [Get a quote](https://api.templarfi.org/docs?spec=swaps#tag/swaps/POST/swaps/quote), [check status](https://api.templarfi.org/docs?spec=swaps#tag/swaps/GET/swaps/status) |
| Bridge | Preparing and tracking NEAR Intents deposits and withdrawals | [Deposits and withdrawals](https://api.templarfi.org/docs?spec=bridge#tag/bridge) |
| Analytics | Warehouse-backed views, protocol state, metrics, and indexed events | [Views](https://api.templarfi.org/docs?spec=analytics#tag/views), [state](https://api.templarfi.org/docs?spec=analytics#tag/state), [metrics](https://api.templarfi.org/docs?spec=analytics#tag/metrics), [events](https://api.templarfi.org/docs?spec=analytics#tag/events) |
| Campaigns | Campaign periods, Merkl data and rewards, and NEAR-to-Stellar payout links | [Campaigns](https://api.templarfi.org/docs?spec=campaigns#tag/campaigns) |
| Compliance | Advisory off-chain wallet screening | [Screen a wallet](https://api.templarfi.org/docs?spec=compliance#tag/compliance/POST/compliance/screen) |
| Relayer | Sponsored submissions, account history, and universal accounts | [Submissions](https://api.templarfi.org/docs?spec=relayer#tag/submissions), [accounts](https://api.templarfi.org/docs?spec=relayer#tag/accounts), [universal accounts](https://api.templarfi.org/docs?spec=relayer#tag/universal-accounts) |

The server base URL shown by the live specifications is `https://api.templarfi.org/v1`. Where an operation requires authentication, its specification marks that requirement and uses the `X-API-Key` header. Use the version displayed by the selected live specification and its downloadable OpenAPI document rather than copying schemas from this guide.

Use this guide for protocol semantics behind the responses: [Risk Parameters](./risk-parameters.md) explains market configuration fields, [Oracles](./oracles.md) explains price provenance, and [Stellar Curated Vaults](./vaults.md) explains vault behavior.

## Rust API Documentation

The generated Rust API documentation, built from the contract sources with `cargo doc`, is the authoritative reference for every contract type, method, argument, and error. It is served under [`/doc/`](/doc/templar_common/index.html) on this site. Useful entry points:

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

Integrators who need transaction-level control without signing NEAR transactions directly can use the gateway service, a JSON-RPC API in front of the contracts. Its method catalog, generated from the service's own method registry, is at [`gateway/METHODS.md`](https://github.com/Templar-Protocol/contracts/blob/dev/gateway/METHODS.md).

## Calling Contracts Directly

Examples throughout this guide use [`near-cli-rs`](./notes.md#contract-interaction-syntax) view calls. The pages that catalogue them:

- [Market](./contract/market/index.md): configuration, positions, snapshots, and the supply, borrow, and liquidate flows.
- [Registry](./contract/registry.md): versions and deployments.
- [Oracles](./oracles.md#inspecting-a-proxy-oracle): proxy oracle feeds, cached prices, and circuit-breaker state.
- [Monitoring and Risk Management](./monitoring.md#protocol-health-checks): the health checks Templar's own monitoring runs.

[templarfi.org](https://templarfi.org/) links to the published guide. As of 2026-09-15, the app navigation does not expose a documentation link; adding one is a frontend change outside this repository.
