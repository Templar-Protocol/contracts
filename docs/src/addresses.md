# Smart Contract Addresses

Deployed Templar Protocol contracts, and how to verify them. For deploying a new
market, see [Deploying a market](./deployments.md).

## Deployments

### Core

| Contract | Account ID | Notes |
|---|---|---|
| Registry | [`v1.tmplr.near`](https://nearblocks.io/address/v1.tmplr.near) | Deploys markets and proxy oracles as sub-accounts; see [Registry](./contract/registry.md) |
| Administrative multisig | [`templar.sputnik-dao.near`](https://nearblocks.io/address/templar.sputnik-dao.near) | Sputnik DAO, 2-of-3 council; see [Protocol Governance](./governance.md) |
| Protocol revenue account | [`revenue.tmplr.near`](https://nearblocks.io/address/revenue.tmplr.near) | Receives the protocol's share of market yield |

### Oracles

| Contract | Account ID | Notes |
|---|---|---|
| Proxy oracle (per market) | `proxy-oracle-<market>.v1.tmplr.near` | Aggregated, circuit-breaker-protected feed for `<market>.v1.tmplr.near` |
| Proxy oracle governance (per market) | `proxy-gov-<market>.v1.tmplr.near` | Timelocked governance for the matching proxy oracle |
| Pyth Lazer adapter | [`pyth-lazer.v1.tmplr.near`](https://nearblocks.io/address/pyth-lazer.v1.tmplr.near) | Verifies and serves signed Pyth Lazer prices |
| RedStone adapter | [`redstone-adapter.v1.tmplr.near`](https://nearblocks.io/address/redstone-adapter.v1.tmplr.near) | Verifies and serves signed RedStone prices |
| Pyth (classic) | [`pyth-oracle.near`](https://nearblocks.io/address/pyth-oracle.near) | Pyth's own contract; read directly by older markets |
| LST oracle adapter | [`lst.oracle.tmplr.near`](https://nearblocks.io/address/lst.oracle.tmplr.near) | Legacy liquid-staking-token adapter |

See [Oracles](./oracles.md) for how these fit together.

### Markets

Market contracts are deployed dynamically through the registry. Each market represents a single asset pair (COLLATERAL &rarr; BORROW) in its own contract account.

Market account names follow `<collateral>-<borrow>[-<n>].v1.tmplr.near`, where each asset name is built from up to three parts: `i` if the token is held through [NEAR Intents](https://docs.near.org/chain-abstraction/intents/overview), the host chain when the asset is not native to it (`eth`, `xlm`, `sol`), and the asset symbol. So `iethwbtc` is WBTC on Ethereum held through Intents, `ixlmusdc` is USDC on Stellar held through Intents, and `ibtc` is native BTC held through Intents. A numeric suffix distinguishes successive deployments of the same pair.

The authoritative list is on-chain:

```bash
near contract call-function as-read-only v1.tmplr.near list_deployments \
    json-args '{"offset": 0, "count": 100}' network-config mainnet now
```

The call is paginated: `count` caps the page size, so if a page comes back full, request the next one with `offset` advanced by the page size (`{"offset": 100, "count": 100}`) and repeat until a page contains fewer than `count` entries.

A selection of available markets:

| Account ID | Collateral Asset | Borrow Asset |
|---|---|---|
| [`ibtc-iethusdc.v1.tmplr.near`](https://nearblocks.io/address/ibtc-iethusdc.v1.tmplr.near) | Native BTC (via NEAR Intents) | USDC on Ethereum (via NEAR Intents) |
| [`ibtc-iethusdc-1.v1.tmplr.near`](https://nearblocks.io/address/ibtc-iethusdc-1.v1.tmplr.near) | Native BTC (via NEAR Intents) | USDC on Ethereum (via NEAR Intents) |
| [`iethwbtc-iethusdc.v1.tmplr.near`](https://nearblocks.io/address/iethwbtc-iethusdc.v1.tmplr.near) | WBTC on Ethereum (via NEAR Intents) | USDC on Ethereum (via NEAR Intents) |
| [`ibtc-usdc-1.v1.tmplr.near`](https://nearblocks.io/address/ibtc-usdc-1.v1.tmplr.near) | Native BTC (via NEAR Intents) | USDC on NEAR |
| [`stnear-usdc-1.v1.tmplr.near`](https://nearblocks.io/address/stnear-usdc-1.v1.tmplr.near) | stNEAR on NEAR | USDC on NEAR |
| [`ixlm-ixlmusdc.v1.tmplr.near`](https://nearblocks.io/address/ixlm-ixlmusdc.v1.tmplr.near) | Native XLM (via NEAR Intents) | USDC on Stellar (via NEAR Intents) |
| [`ixlm-ixlmpyusd.v1.tmplr.near`](https://nearblocks.io/address/ixlm-ixlmpyusd.v1.tmplr.near) | Native XLM (via NEAR Intents) | PYUSD on Stellar (via NEAR Intents) |
| [`iethfxrp-ixlmusdc.v1.tmplr.near`](https://nearblocks.io/address/iethfxrp-ixlmusdc.v1.tmplr.near) | FXRP on Ethereum (via NEAR Intents) | USDC on Stellar (via NEAR Intents) |
| [`ixlmsolvbtc-ixlmusdc.v1.tmplr.near`](https://nearblocks.io/address/ixlmsolvbtc-ixlmusdc.v1.tmplr.near) | SolvBTC on Stellar (via NEAR Intents) | USDC on Stellar (via NEAR Intents) |
| [`ixlmustry-ixlmusdc.v1.tmplr.near`](https://nearblocks.io/address/ixlmustry-ixlmusdc.v1.tmplr.near) | USTRY on Stellar (via NEAR Intents) | USDC on Stellar (via NEAR Intents) |
| [`ixlmcetes-ixlmusdc.v1.tmplr.near`](https://nearblocks.io/address/ixlmcetes-ixlmusdc.v1.tmplr.near) | CETES on Stellar (via NEAR Intents) | USDC on Stellar (via NEAR Intents) |
| [`ixlmdejaaa-ixlmusdc-2.v1.tmplr.near`](https://nearblocks.io/address/ixlmdejaaa-ixlmusdc-2.v1.tmplr.near) | deJAAA on Stellar (via NEAR Intents) | USDC on Stellar (via NEAR Intents) |
| [`ixlmdejtrsy-ixlmusdc.v1.tmplr.near`](https://nearblocks.io/address/ixlmdejtrsy-ixlmusdc.v1.tmplr.near) | deJTRSY on Stellar (via NEAR Intents) | USDC on Stellar (via NEAR Intents) |

Each market's oracle account, price identifiers, and risk parameters are available from its `get_configuration` view; see [Market Configuration](./contract/market/index.md#configuration). The declarative specifications markets were deployed from are in [`deployments/v1/`](https://github.com/Templar-Protocol/contracts/tree/dev/deployments/v1).

A separate registry, [`templar-alpha.near`](https://nearblocks.io/address/templar-alpha.near), hosts pre-release and liquidation-test markets on mainnet. Markets under it are not production markets.

### Contract Verification

All smart contracts use reproducible builds. To verify deployed code:

```bash
near contract verify deployed-at <contract-id> mainnet now
```

Example output:

```txt
INFO The code obtained from the contract account ID and the code calculated from the repository are the same.
|    Contract code hash: DaudmUa3nAym9dfQkn8mpNPZxkphSRGwEaTMgtymVhFE
|    Contract version:	1.0.0
|    Standards used by the contract:	[nep330:1.2.0]
|    View the contract's source code on:	https://github.com/Templar-Protocol/contracts/tree/1d736e62a86424dd947284cbd8e83bef803fa9fb
|    Build Environment:	sourcescan/cargo-near:0.13.4-rust-1.85.0@sha256:a9d8bee7b134856cc8baa142494a177f2ba9ecfededfcdd38f634e14cca8aae2
|    Build Command:	cargo near build non-reproducible-wasm --locked
```

The verification compares the on-chain code hash with a build from the commit recorded in the contract's NEP-330 metadata, so it establishes that the deployed bytecode matches the published source commit. Whether that commit is covered by an audit must be checked separately against the audit reports. Released contract artifacts and their SHA-256 digests are recorded under [`contract/artifacts/releases/`](https://github.com/Templar-Protocol/contracts/tree/dev/contract/artifacts/releases).
