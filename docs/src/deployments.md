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
| [`ixlm-ixlmusdc.v1.tmplr.near`](https://nearblocks.io/address/ixlm-ixlmusdc.v1.tmplr.near) | Native XLM (via NEAR Intents) | USDC on XLM (via NEAR Intents) |

## Signing

Most `tmplrmgr` writes take `--signer-id` plus one credential, and `--sign-with`
selects where that signing key lives:

| Backend | Credential | Notes |
|---|---|---|
| `secret-key` (default) | `--secret-key`, or `$SECRET_KEY` | Puts a plaintext key in the environment. Fine for testnet and CI; avoid for mainnet. |
| `keychain` | the OS keychain | Looked up by account id. The account's on-chain keys are listed to find a match. |
| `ledger` | a Ledger device | Uses near-api's default HD path. The device must be unlocked with the NEAR app open. |

For mainnet, prefer to sign nothing directly. `--print sputnik` emits a
SputnikDAO proposal instead of executing, so a deployment can be reviewed and
approved by the multisig with no operator key involved at all:

```bash
tmplrmgr market create --signer-id dao.near --print sputnik --public-key ed25519:… …
```

`keychain` and `ledger` hold their key outside this process, so writes that
embed the signer's public key on a new account (any `registry deploy`) need it
passed explicitly with `--public-key`.

`registry clear-deployments` is the exception: it signs many *discovered*
accounts with one authorized key, so it has no `--signer-id` and takes only
`--secret-key`.

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
