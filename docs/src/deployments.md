# Smart Contract Deployments

This page provides information about Templar Protocol smart contracts and how to interact with them.

## Deployments

- **Registry**: [`v1.tmplr.near`](https://nearblocks.io/address/v1.tmplr.near)
- **LST Oracle Adapter**: [`lst.oracle.tmplr.near`](https://nearblocks.io/address/lst.oracle.tmplr.near)

### Markets

Market contracts are deployed dynamically through the registry. Each market represents a single asset pair (COLLATERAL &rarr; BORROW).

A selection of available markets is shown below:

| Account ID | Collateral Asset | Borrow Asset |
|---|---|---|
| [`ibtc-iethusdc.v1.tmplr.near`](https://nearblocks.io/address/ibtc-iethusdc.v1.tmplr.near) | Native BTC (via NEAR Intents) | USDC on Ethereum (via NEAR Intents) |
| [`iethwbtc-iethusdc.v1.tmplr.near`](https://nearblocks.io/address/iethwbtc-iethusdc.v1.tmplr.near) | wBTC on Ethererum (via NEAR Intents) | USDC on Ethereum (via NEAR Intents) |
| [`ibtc-usdc-1.v1.tmplr.near`](https://nearblocks.io/address/ibtc-usdc-1.v1.tmplr.near) | Native BTC (via NEAR Intents) | USDC on NEAR |
| [`stnear-usdc-1.v1.tmplr.near`](https://nearblocks.io/address/stnear-usdc-1.v1.tmplr.near) | stNEAR on NEAR | USDC on NEAR |

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
