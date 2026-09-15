# Registry

The registry is a contract that maintains a list of contract versions, deploys new contracts, and maintains a list of those deployments. It is the only way a production market or proxy oracle is created: every one is a sub-account of the registry, deployed from a registered, audited version.

The account ID of the mainnet market registry is [`v1.tmplr.near`](https://nearblocks.io/address/v1.tmplr.near). Its owner is the DAO multisig; what the owner can and cannot do is described in [Protocol Governance](../governance.md#registry-contract).

## Interactions

Every example below is a read-only view call with [`near-cli-rs`](../notes.md#contract-interaction-syntax). The outputs are illustrative and abridged; the live call is authoritative.

### List available versions

```bash
near contract call-function as-read-only \
    v1.tmplr.near list_versions \
    json-args '{"offset":0,"count":100}' \
    network-config mainnet \
    now
```

Illustrative output:

```json
[
  "v1.0.0",
  "v1.1.0",
  "v1.3.0",
  "templar-proxy-oracle-near-contract@0.4.1#fb9b3b46bbedd16665dbc31f0efc605c8ef49acfa5ff17608fbc3b490b4a6cb3",
  "templar-proxy-oracle-near-governance-contract@0.3.1#40b1719a525dae7117a776ccefa281f97470e1213e83bbf054bd77eae5b0568a"
]
```

Market versions are registered under `vX.Y.Z` keys; proxy oracle and governance builds under `<package>@<version>#<sha256>` keys. The mainnet deployment profile currently deploys new markets from `v1.3.0` and pins the proxy oracle and governance builds shown above; other keys may exist. Which release each key corresponds to is recorded in the [Deployment and Version Log](../release-log.md).

### List deployments

```bash
near contract call-function as-read-only \
    v1.tmplr.near list_deployments \
    json-args '{"offset":0,"count":100}' \
    network-config mainnet \
    now
```

Illustrative output:

```json
[
  "ibtc-iethusdc-1.v1.tmplr.near",
  "proxy-oracle-ibtc-iethusdc-1.v1.tmplr.near",
  "proxy-gov-ibtc-iethusdc-1.v1.tmplr.near",
  "ixlm-ixlmusdc-1.v1.tmplr.near",
  "proxy-oracle-ixlm-ixlmusdc-1.v1.tmplr.near",
  "proxy-gov-ixlm-ixlmusdc-1.v1.tmplr.near",
  "ixlmdejaaa-ixlmusdc-2.v1.tmplr.near",
  "..."
]
```

The list contains markets, proxy oracles, and proxy oracle governance contracts alike, since the registry deploys all three, and it includes deprecated markets that are still on chain. The call is paginated: `count` caps the page size, so if a page comes back full, request the next one with `offset` advanced by the page size and repeat until a page contains fewer than `count` entries. The markets currently offered in the app, and the deprecated ones, are listed on [Smart Contract Addresses](../addresses.md#markets).

### Read a deployment record

```bash
near contract call-function as-read-only \
    v1.tmplr.near get_deployment \
    json-args '{"account_id":"ixlmdejaaa-ixlmusdc-2.v1.tmplr.near"}' \
    network-config mainnet \
    now
```

Illustrative output:

```json
{
  "version_key": "v1.3.0",
  "code_hash": "<base58 sha256 of the deployed WASM>",
  "block_height": "…"
}
```

`version_key` names the registered version the account was deployed from and `code_hash` is the hash of the bytes deployed; compare it with the release catalog in the [Deployment and Version Log](../release-log.md#verifying-a-deployed-contract-against-the-catalog). The record's types are documented in the [API Reference](../api-reference.md).
