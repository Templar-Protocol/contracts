# Stellar Curated Vaults

Curated vaults let depositors earn lending yield without managing positions themselves. A depositor supplies one asset to a vault on Stellar and receives transferable shares; a professional curator decides which markets the pooled assets are lent into and under what limits, and an independent Sentinel can halt the vault at any time. This page explains how a vault works from the depositor's side. Operators should read the [Stellar Vault Curator Guide](./curator-guide.md); the governance model is summarized in [Protocol Governance](./governance.md#curated-vault-governance).

## Overview

A vault accepts a single [SEP-41](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0041.md) asset (for example USDC on Stellar) and issues SEP-41 vault shares in return. The vault exposes ERC-4626-style `deposit`, `mint`, `withdraw`, `redeem`, and preview methods through a Soroban proxy, so wallets and integrators can treat it like any tokenized vault.

The core accounting identity is:

```text
total_assets = idle_assets + external_assets
```

- **Idle assets** are tokens the vault holds directly. They pay out deposits, atomic withdrawals, and queued withdrawals.
- **External assets** are the value of the vault's positions in markets, reported through one adapter per market route (a Blend pool adapter, or a custodial adapter for off-chain routes).
- **Share price** is `total_assets / total_shares` (with small virtual offsets that make the first deposit's price manipulation-resistant). Yield from markets raises `external_assets` and therefore the share price; losses lower it.

Adapter values are refreshed by the vault's operators rather than read live on every call, so a preview can lag a market move until the next refresh.

## Depositing

`deposit` takes assets and mints shares at the current share price; `mint` asks for an exact number of shares. Both are atomic. Deposits can be limited by a vault-wide cap and by restrictions (a whitelist or blacklist of accounts) that governance or the Sentinel may apply. Preview methods (`preview_deposit`, `preview_mint`) return the expected outcome before signing.

## Withdrawing

There are two exit paths, and the difference matters:

- **Atomic withdrawal** (`withdraw`, `redeem`, and the slippage-protected `atomic_withdraw` / `atomic_redeem`) settles in one transaction from **idle assets only**. It never pulls liquidity back from a market. `max_withdraw` and `max_redeem` can therefore be zero while your shares are fully backed by assets deployed in markets.
- **Queued withdrawal** (`request_withdraw`) escrows your shares and records a fixed asset claim at the share price at request time. After a cooldown (one hour by default; the vault's actual value is a governance parameter) an allocator recalls liquidity from markets if needed and executes the queue head. Requests are paid in order, in full or not at all; there is no partial payout and no user-side cancellation. The queue does not reserve idle assets against atomic exits, so curators are expected to keep enough idle liquidity for outstanding claims.

There is no forced exit mechanism: a vault cannot compel a market to return liquidity faster than the market's own withdrawal rules allow.

## Fees

Vaults may charge two fees, both paid by minting new shares to a fee recipient (which dilutes existing holders rather than moving tokens):

| Fee | Basis | Maximum |
|---|---|---|
| Management | Time-weighted on assets under management, accrued regardless of performance | 5% per year |
| Performance | Growth in assets since the last fee checkpoint; zero in flat or losing periods | 50% of profit |

Two details depositors should understand:

- The performance fee is measured **since the last checkpoint, not above an all-time high-water mark**. After a loss, the recovery back to the previous level is chargeable.
- A **growth-rate cap** can limit how quickly assets are allowed to count towards fee accrual, which bounds the fee impact of a sudden inflow or a mis-reported adapter value.

Fee increases and recipient changes are timelocked; decreases execute immediately.

## Caps and Cap Groups

The curator sets a per-market cap on how much of the vault may be allocated to each route, and can group correlated routes under a shared **cap group** with an absolute limit, a relative limit (a share of total assets), or both. The effective ceiling for a group is the lower of the two. Raising a cap, adding a market, or changing group membership is timelocked; lowering a cap, including to zero, is immediate.

## Roles and Addresses

Every vault has four roles. The addresses that hold them are specific to each vault and published in that vault's own documentation (see the [live example](#live-example-bizantine-labs-tbizusdc-core) below); verify them on chain before relying on them.

| Role | Can | Cannot | Address |
|---|---|---|---|
| Governance admin | Submit and accept governance proposals: caps, fees, roles, restrictions, upgrades, timelock changes. May be a Stellar account or a multisig contract. | Bypass a timelock; pause (only the Sentinel can). | See the vault's documentation |
| Curator | Set allocation policy within governance-approved caps; act as an allocator. | Raise a cap or add a market without the timelock. | See the vault's documentation |
| Allocator | Move assets between the vault and its markets, refresh adapter values, execute ready queued withdrawals, abort a stuck operation. | Change policy or fees. | See the vault's documentation |
| Sentinel | Pause the vault, tighten restrictions, revoke pending operational proposals, and use emergency recovery, all immediately. | Unpause, relax restrictions, or accept proposals. | See the vault's documentation |

Timelocks are configured per action kind between 0 and 30 days, chosen at deployment. As a rule, **risk-increasing changes are timelocked and risk-reducing changes are immediate**: unpausing, fee increases, cap increases, new markets, role changes, and upgrades wait; pausing, fee decreases, cap decreases, and timelock increases do not. The governance admin can permanently disable an action kind (`abdicate`), which is irreversible. The full rules are in the curator guide's [exact timing rules](./curator-guide.md#exact-timing-rules).

## Risks

- **Market risk**: the vault's assets are lent into markets; a market's bad debt or liquidity shortfall reduces `external_assets` and the share price.
- **Adapter and custody risk**: a Blend adapter exposes the vault to the Blend pool it targets. A custodial adapter forwards assets to a custodian, and its reported value is set by that custodian, so the custodian's controls, reporting cadence, and liquidity-return procedure are inside the trust boundary. The custodial adapter was out of scope of the Halborn vault audit.
- **Liquidity risk**: atomic exits depend on idle liquidity; queued exits depend on allocators recalling funds and on the markets' own withdrawal rules.
- **Governance and key risk**: a vault deployed with zero-duration timelocks gives its admin immediate control over every parameter. Check the timelocks in force before depositing. Vault runtimes are upgradeable through governance under the upgrade timelock; verify the controlling addresses.
- **Fee risk**: see the checkpoint note under [Fees](#fees).

## Checking a Vault

Anyone can read a vault's state through its ERC-4626 proxy with the Stellar CLI:

```bash
# Total assets under management and shares outstanding
stellar contract invoke --id <vault-4626-proxy-id> --network mainnet -- total_assets
stellar contract invoke --id <vault-4626-proxy-id> --network mainnet -- total_supply

# The underlying asset contract, and what one share is worth
stellar contract invoke --id <vault-4626-proxy-id> --network mainnet -- asset
stellar contract invoke --id <vault-4626-proxy-id> --network mainnet -- convert_to_assets --shares 10000000

# How much can be withdrawn atomically right now for an owner
stellar contract invoke --id <vault-4626-proxy-id> --network mainnet -- max_withdraw --owner <G...>
```

Governance state (the proposal queue, timelocks, and roles) is read from the vault's governance contract; the curator guide shows the `tmplr-soroban-vault governance queue` and `governance explain` commands for that. Vault events (deposits, withdrawals, allocations, fee accruals, pauses) are emitted on chain and are part of the [monitoring](./monitoring.md) coverage.

## Live Example: Bizantine Labs tBizUSDC-CORE

[Bizantine Labs](https://docs.bizantinelabs.xyz/vaults/tbizusdc-core/overview) publishes a complete specification for **tBizUSDC-CORE**, a USDC vault on Stellar built on Templar's Soroban vault framework. Its configuration illustrates useful vault controls: a separate Sentinel and NAV reporter, explicit withdrawal and refresh cooldowns, directional governance timelocks, and caps for every route.

The values below are the configuration Bizantine publishes. Verify the live contracts before depositing. Bizantine also discloses that the combined admin, curator, and adapter-admin role is currently a single Fordefi-custodied key, adapter upgrades have no on-chain timelock, and independent reconciliation of the full address set remains a pre-funding gate.

### Configuration

| Parameter | Published value |
|---|---|
| Deposit asset | USDC on Stellar |
| Management fee | 0% |
| Performance fee | 15% |
| Growth-rate cap | 10% per year |
| Queued-withdrawal cooldown | 1 hour |
| Idle-resync cooldown | 120 seconds |
| Governance timelock | 24 hours, directional: risk-increasing changes wait; risk-reducing changes are immediate |
| Restrictions | None; deposits are open, so the $1 million pilot cap is an operational limit rather than an on-chain deposit cap |
| Target allocation | 10% idle, 20% Blend, 40% BTC, 25% XLM, 2.5% XRP, 2.5% ZEC |
| Aggregate Templar-market cap | 70% of NAV |

Source: Bizantine's [Core Parameters](https://docs.bizantinelabs.xyz/vaults/tbizusdc-core/overview#core-parameters), [Sleeve Design](https://docs.bizantinelabs.xyz/vaults/tbizusdc-core/overview#sleeve-design), and [Operating Controls](https://docs.bizantinelabs.xyz/vaults/tbizusdc-core/overview#operating-controls).

### Roles

| Role | Stellar account | Published control |
|---|---|---|
| Admin, curator, and adapter admin | `GAXHOW2QMS2R3OD6MPSWMBE3XVYZEL5BFGG2BBPTINP4D37MGF64IM2P` | Single Fordefi-custodied key; governance proposals, allocation policy, and adapter administration |
| Sentinel | `GAHJUZUQ6D3YUJTY4LPMMQCPNEJW46LA3GUNSOUWPDF5EZLDI3AMKPTW` | Separately custodied; may pause and tighten restrictions, but cannot relax them or control adapter upgrades |
| Custodian and NAV reporter | `GBCF7WMYE6KUOKU5DHCZBIWHD6UANXI6XR25FNKPKSVKBDZUHV3MEBGK` | Separate Fordefi vault; reports assets for the custodial routes |
| Allocator | `GBZCCMTR4I3MOIQM4TOLDU3PNONUQLWXTIZGZ5FVOZDNMJ7UG7RMKTUE` | Executes allocator and rebalancer transactions; shares the admin's Fordefi vault |
| TTL keeper | `GC4JT5SKX5MKDYMCXNPXQCTZ73FRCCBTKMQRUJ7MYZXJR46WFRDHDMD3` | Low-privilege account used only to extend contract TTLs |

Source: Bizantine's [Roles and Addresses](https://docs.bizantinelabs.xyz/vaults/tbizusdc-core/overview#roles-and-addresses).

### Core Contracts

| Contract | Stellar contract ID |
|---|---|
| Vault | `CBH7TKSKKYF2AXSMRUCM5RATSB22PVZPBVWN6ZDCHCH37TKR3DKE4XAA` |
| Share token | `CCZF6EUIP2ZYSDX6SXMJCIYDQBXKJWMFJ6OJCYBGFZVT4RSK67JIQ2GW` |
| Governance | `CBPN2FYGGUFUYL73NAIQOJTVGGDZLHW6D4L3M2ING4NPYPHPZS2QK3TU` |
| ERC-4626 proxy | `CB6XEPVIX4GGQC7M7WU3JI4VCJEDH43SCIWZY6265S3LMZQGOXDUTGX5` |
| Curator proxy | `CCBFW24W4K3D6M6IXE4PDYN262B3H4FKJ3YDGJVDQREGNFZ6ICCB7GGV` |
| USDC asset contract | `CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75` |
| Blend Capital USDC pool | `CAJJZSGMMM3PD7N33TAPHGBUGTB43OC73HVIK2L2G6BNGGGYOSSYBXBD` |

### Routes and Caps

| Route | Adapter contract | Target allocation | Published cap |
|---|---|---|---|
| Blend Capital USDC | `CDWB5P44ESHPS47F4USHICZ4O2IPKCBCRTDDWFXVZZDP7H43MTBQQQZO` | 20% | 400,000 USDC and 40% of NAV |
| Templar BTC/USDC | `CBGZL4EHE77GJ23VD47WDRC4XBXTNFQPG3PSXTQJBGPVTK4WYPRWFVDP` | 40% | 400,000 USDC and 40% of NAV |
| Templar XLM/USDC | `CC2WFNZPXZHKTPFCPWLFXGDLEV2YD344MEN72XGUUMXUHY7PDDTCA5TV` | 25% | 250,000 USDC and 25% of NAV |
| Templar XRP/USDC | `CCWURNFYN4OHUYDKQGLOCBZ4ABX2ILMVFGT6V4UJ2SGF3FY7ZNPO7IWH` | 2.5% | 25,000 USDC and 2.5% of NAV |
| Templar ZEC/USDC | `CAVFJ5AETBAC52ZNUGIWJSLOHPT43S2HPP4OAFWNBPYINOK4IOTFWR7K` | 2.5% | 25,000 USDC and 2.5% of NAV |

The four Templar routes use custodial adapters: assets away from Stellar are represented by signed NAV reports, so the custodian and reporting process remain inside the trust boundary. Source: Bizantine's [Protocol Address Appendix](https://docs.bizantinelabs.xyz/vaults/tbizusdc-core/overview#protocol-address-appendix) and [Sleeve Design](https://docs.bizantinelabs.xyz/vaults/tbizusdc-core/overview#sleeve-design).
