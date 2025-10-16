# Registry

The registry is a contract that maintains a list of contract versions, deploys new contracts, and maintains a list of those deployments.

Market contracts are deployed through a registry, and thus appear as its subaccounts.

The account ID of the mainnet market registry is [`v1.tmplr.near`](https://nearblocks.io/address/v1.tmplr.near).

## Interactions

### List available versions

```bash
near contract call-function as-read-only \
    v1.tmplr.near list_versions \
    json-args '{"offset":0,"count":100}' \
    network-config mainnet \
    now
```

Output:

```json
[
  "v1.0.0",
  "v1.1.0"
]
```

### List deployments

```bash
near contract call-function as-read-only \
    v1.tmplr.near list_deployments \
    json-args '{"offset":0,"count":100}' \
    network-config mainnet \
    now
```

Output:

```json
[
  "ibtc-usdc.v1.tmplr.near",
  "stnear-usdc.v1.tmplr.near",
  "ibtc-iethusdc.v1.tmplr.near",
  "iethwbtc-iethusdc.v1.tmplr.near",
  "ibtc-usdc-1.v1.tmplr.near",
  "stnear-usdc-1.v1.tmplr.near"
]
```
