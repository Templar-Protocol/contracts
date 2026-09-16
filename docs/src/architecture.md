# Architecture Overview

Templar is an overcollateralized lending protocol whose markets run on NEAR. Collateral and borrow assets that live on other chains reach those markets through [NEAR Intents](https://docs.near.org/chain-abstraction/intents/overview), prices reach them through proxy oracles that aggregate independent providers, and depositors on Stellar reach them through curated vaults. This page shows how the pieces fit together and what each external dependency is trusted to do.

## System Map

```text
                 Users, wallets, integrators
                            |
          +-----------------+------------------+
          |                                    |
   app.templarfi.org                    Gateway (JSON-RPC)
   (frontend)                           Relayer (gasless txs)
          |                                    |
          +-----------------+------------------+
                            v
 NEAR ..........................................................................
 :                                                                            :
 :   intents.near  (NEP-245 multi-token balances of bridged assets)           :
 :        |  deposits / withdrawals via NEAR Intents                          :
 :        v                                                                   :
 :   Registry v1.tmplr.near --deploys--> Markets <pair>.v1.tmplr.near         :
 :        |                                  |  reads prices                  :
 :        |                                  v                                :
 :        +-------deploys-------> Proxy oracle proxy-oracle-<pair>            :
 :                                   ^   ^   |  owned by                      :
 :            Pyth Lazer adapter ----+   |   v                                :
 :            RedStone adapter ----------+  Governance proxy-gov-<pair>       :
 :            Pyth (classic) -------------+  (timelocked; Admin = DAO)        :
 :            LST oracle adapter ---------+         ^                         :
 :                                                  |                         :
 :                            DAO multisig templar.sputnik-dao.near (2-of-3)  :
 :............................................................................:

 Stellar .......................................................................
 :                                                                            :
 :   Depositor --> ERC-4626 proxy --> Vault runtime --> Adapters --> Markets   :
 :                                     |   ^              (Blend pool,        :
 :                          Share token |   | Governance    custodial route)   :
 :                          (SEP-41)    v   | + Sentinel                      :
 :                                   Curator proxy                            :
 :............................................................................:

 Off-chain services: liquidator, accumulator, market-monitor, redstone-bridge,
 relayer, gateway, funding-bridge, plus Hypernative and custom alerting.
```

## Components

### NEAR contracts

| Component | Role | Mutability | More |
|---|---|---|---|
| [Registry](./contract/registry.md) `v1.tmplr.near` | Holds audited contract versions and deploys markets and proxy oracles as its sub-accounts; records each deployment's version key and code hash. | Owned by the DAO multisig; can register versions and upgrade itself, cannot touch deployed contracts. | [Governance](./governance.md#registry-contract) |
| [Markets](./contract/market/index.md) `<collateral>-<borrow>[-n].v1.tmplr.near` | One collateral asset, one borrow asset, one contract. Supply, borrow, repay, withdraw, liquidate; interest accrual through periodic snapshots. | No admin methods, no upgrade, no pause. Parameters fixed at deployment. | [Risk Parameters](./risk-parameters.md), [Security](./security-overview.md#immutable-markets) |
| [Proxy oracles](./oracles.md#proxy-oracle) `proxy-oracle-<market>` | Aggregate Pyth Lazer, Pyth classic, RedStone, and LST-transformer sources per feed with freshness filters, weighted medians, and circuit breakers; the market's only price input. | Configurable and upgradeable through its governance contract. | [Oracles](./oracles.md) |
| Proxy oracle governance `proxy-gov-<market>` | Timelocked owner of a proxy oracle; every configuration or code change is a maturing proposal. Breaker operation is immediate in both directions: `ManualTripper` can trip or untrip a feed and `CircuitBreakerOperator` can re-arm a breaker and enable or disable enforcement with no delay, so untripping, re-arming, and disabling enforcement are zero-delay risk-increasing actions, held by the DAO multisig and alerted. | Admin role held by the DAO multisig. | [Governance](./governance.md#proxy-oracle-governance) |
| Oracle adapters `pyth-lazer.v1.tmplr.near`, `redstone-adapter.v1.tmplr.near`, `lst.oracle.tmplr.near` | Verify provider signatures and serve prices to proxy oracles. `pyth-oracle.near` is Pyth's own contract. | Owned; signer sets are admin-managed by the DAO multisig. | [Oracle Addresses](./oracles.md#oracle-addresses) |
| DAO multisig `templar.sputnik-dao.near` | 2-of-3 Sputnik DAO that administers every mutable NEAR component. | Signer changes are themselves DAO proposals. | [Administrative Multisig](./governance.md#administrative-multisig) |

### Stellar vault stack

A curated vault is a set of Soroban contracts: the **vault runtime** (custody, accounting, withdrawal queue, roles), a **share token** (SEP-41), a **governance contract** (per-action timelocks, Sentinel), an **ERC-4626 proxy** (the depositor-facing interface), a **curator proxy**, and one **adapter** per market route (a Blend pool adapter or a custodial adapter). The runtime and the proxy oracle share chain-agnostic Rust kernels with their NEAR counterparts, so the accounting logic is written and verified once. See [Stellar Curated Vaults](./vaults.md) and the [Curator Guide](./curator-guide.md).

### Off-chain services

None of these hold a privileged key over a market. They provide liveness and convenience; if all of them stopped, users could still interact with every contract directly.

| Service | Purpose |
|---|---|
| [Gateway](https://github.com/Templar-Protocol/contracts/tree/dev/gateway) | JSON-RPC API for integrators in front of the contracts; see [API Reference](./api-reference.md#gateway-json-rpc). |
| [Relayer](https://github.com/Templar-Protocol/contracts/tree/dev/service/relayer) | Relays signed delegate actions and pays their gas, so accounts without NEAR can transact. |
| [Liquidator](https://github.com/Templar-Protocol/contracts/tree/dev/service/liquidator) | Scans positions and liquidates undercollateralized ones. Liquidation is permissionless; this is Templar's own liquidator, not the only one. |
| [Accumulator](https://github.com/Templar-Protocol/contracts/tree/dev/service/accumulator) | Applies interest to borrow positions by triggering market snapshots. |
| [Market monitor](https://github.com/Templar-Protocol/contracts/tree/dev/service/market-monitor) | Position-health scanning with alerts to the public channel; part of [monitoring](./monitoring.md). |
| [RedStone bridge](https://github.com/Templar-Protocol/contracts/tree/dev/service/redstone-bridge) | Delivers signed RedStone price payloads to the RedStone adapter. |
| [Funding bridge](https://github.com/Templar-Protocol/contracts/tree/dev/service/funding-bridge) | Treasury operations across chains through the NEAR Intents bridge API. |

## Flows

### How cross-chain collateral reaches a market

1. A user deposits an asset on its home chain (Bitcoin, Ethereum, Stellar, XRP Ledger, Solana, and others) through NEAR Intents. The bridged balance is credited to the user on `intents.near` as a NEP-245 multi-token.
2. Native-chain assets are represented as `<chain>.omft.near` tokens (for example `btc.omft.near`, `eth-0xa0b8….omft.near` for USDC on Ethereum). Stellar-origin assets (XLM, USDC on Stellar, PYUSD, deJAAA, deJTRSY, SolvBTC, CETES, USTRY) are represented through the HOT Labs omni bridge as `v2_1.omni.hot.tg:1100_…` tokens. The full identifiers are on [Risk Parameters](./risk-parameters.md#asset-identifiers).
3. The user transfers the token from their `intents.near` balance to the market with a `mt_transfer_call`; the market records the collateral (or supply) position. Markets never custody assets on other chains; everything they hold is a NEP-245 balance on NEAR.
4. Withdrawals reverse the path: the market transfers the token back to the user's `intents.near` balance, and NEAR Intents settles it to the destination chain.

### Borrowing, repayment, and liquidation

1. Before a price-dependent operation (borrow, collateral withdrawal against debt, liquidation), the caller or a bot refreshes the market's proxy oracle. The proxy pulls fresh prices from its sources, filters stale ones, aggregates, runs circuit-breaker checks, and caches the accepted price.
2. The market reads the accepted price with a freshness bound (`price_maximum_age`) and applies its immutable collateralization rules; see [Risk Parameters](./risk-parameters.md).
3. If a position falls below the liquidation MCR, any account may repay part of its debt and receive collateral at up to the market's maximum liquidation spread.
4. Supply withdrawals, repayments, and collateral withdrawals from debt-free positions never depend on the oracle, so user exits survive an oracle outage or a tripped breaker.

### How Stellar deposits reach markets

1. A depositor deposits USDC (or another supported asset) into a vault's ERC-4626 proxy on Stellar and receives shares.
2. The curator's allocation policy, within governance-approved caps, directs allocators to supply pooled assets into market routes through the vault's adapters.
3. Yield accrues to `external_assets` and is reflected in the share price. Withdrawals are paid from idle liquidity or, through the withdrawal queue, after allocators recall liquidity from markets. See [Stellar Curated Vaults](./vaults.md#withdrawing).

## Trust Assumptions

| Dependency | What Templar relies on it for | What it cannot do | Assurance |
|---|---|---|---|
| NEAR Intents (`intents.near`) | Custody of every bridged collateral and borrow asset while it is on NEAR; settlement of deposits and withdrawals to other chains. | It has no role in market logic; a market's balances are NEP-245 entries it cannot reprice. | Audited independently (Hacken, Guvenkaya, zkSecurity, Aurora Labs); see [Dependency Audits](./security-overview.md#dependency-audits). Status: [status.near-intents.org](https://status.near-intents.org/). |
| Omnibridge and Chain Signatures | The bridging and MPC signing behind NEAR Intents transfers. | Cannot act on Templar contracts. | Omnibridge and NEAR One MPC audits in the same folder. |
| HOT Labs omni bridge (`v2_1.omni.hot.tg`) | Representation of Stellar-origin assets on NEAR. | Cannot act on Templar contracts. | HOT publishes [bridge architecture documentation](https://docs.hotdao.ai/white-paper/hot-bridge), and its [EVM MetaWallet repository](https://github.com/hot-dao/omni-wallet-solidity) states that component was audited by Hacken. No public report was found that establishes audit coverage for the deployed NEAR `v2_1` balance contract or the Stellar locker. |
| Pyth (Lazer and classic) | Signed price updates for most feeds; the higher-weighted source in the standard configuration. | Cannot bypass the freshness filter or the circuit breakers; a single stale or blocked source reads as no price rather than a wrong one. | Signer sets verified on chain by the adapter; provider status at [status.pyth.network](https://status.pyth.network/). |
| RedStone | Signed price updates; the second source in the standard configuration and the sole source for some fundamental-value feeds. | As above. | Signer threshold verified on chain by the adapter. |
| Blend (Stellar) | Lending venue behind Blend-adapter vault routes. | Cannot affect NEAR markets; exposure is limited by the vault's caps. | Blend's own audits; Templar's adapter is in the Halborn vault audit scope. |
| Custodians (custodial adapter routes) | Off-chain deployment of vault assets and reporting of their value. | Reporting is bounded by nonce and expected-value checks; exposure is capped per route. | Out of scope of the Halborn vault audit; each route's controls are documented by the curator. |
| Templar's own bots | Liveness: price refreshes, liquidations, interest accrual, alerts. | Hold no privileged role; every action they take is permissionless. | Monitored; see [Monitoring and Risk Management](./monitoring.md#operational-services). |
| DAO multisig signers | Governance of proxy oracles, adapters, the registry, and retained deployer keys. | Cannot change a market's parameters or move user funds through any contract method; storage patches are limited to accounts whose deployer key is still retained. | 2-of-3 threshold, timelocked proposals, alerted actions; see [Governance](./governance.md). |

Historical design diagrams and papers are available in the public [`Templar-Protocol/architecture` repository](https://github.com/Templar-Protocol/architecture). That repository is somewhat outdated; use this guide, the current contract source, and deployed configuration as the authoritative description of the current system.
