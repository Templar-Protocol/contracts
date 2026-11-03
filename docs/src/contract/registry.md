# Registry

The registry is a contract that maintains a list of contract versions, deploys new contracts, and maintains a list of those deployments. It is the only way a production market or proxy oracle is created: every one is a sub-account of the registry, deployed from a registered, audited version.

The account ID of the mainnet market registry is [`v1.tmplr.near`](https://nearblocks.io/address/v1.tmplr.near). Its owner is the DAO multisig; what the owner can and cannot do is described in [Protocol Governance](../governance.md#registry-contract).

Role-enabled V2 separates staged uploads from approved versions: Uploader pays
actual storage for `upload_code` (Borsh bytes), Owner approves an Uploaded or
Global hash with JSON, and Deployer creates accounts from approved versions.
Owner implicitly has both roles and alone grants/revokes them, removes version
code, upgrades the registry, or uses legacy `add_version` (including
`PublishGlobal`). Query live owner/role/state-version views before assuming a
registry has migrated; see the [gated rollout](../deployments.md#role-enabled-v2-rollout-alpha-then-v1).

Deployment names `probe` and `deploy` are reserved for implementation-owned
scratch accounts used by Global probing and publication. All authorized callers,
including Owner, receive `Deployment name is reserved`; Deployer authority cannot
occupy either scratch account. Other deployment names retain the normal role checks.

Uploads are single-use by hash: duplicate or already-approved hashes are refused,
including soft-deleted versions and in-flight Global probes. Owner approval
moves Uploaded bytes without copying or refunding their storage charge. Only a
successful Global probe consumes a matching upload; failure preserves it and
frees the version key for retry. Successfully registered keys remain occupied
permanently. A current Uploader can remove only its own unapproved uploads;
Owner can remove any, without refund.

`get_upload` returns only `{uploader, code_len}`, never bytes; `list_uploads`
lists hashes. `list_uploads` and `list_role_members` accept `count`/`offset` pagination.
`has_role` includes implicit Owner authority, while membership lists show
explicit grants only. Propose/accept ownership transfer preserves explicit grants;
`own_renounce_owner` is absent. V1→V2 migration moves existing map handles and
creates empty uploads; it does not rewrite historical entries or owner slots.

V2 keeps iterable upload metadata at prefix `b"u"` and each lazy code blob at
`b"c" + code_hash`. Removing one upload therefore does not deserialize or rewrite
an unrelated upload's bytes when the iterable map moves its tail entry. Upload
storage charging includes both metadata and the separately flushed blob.
This V2 layout is not yet published, so this correction needs no deployed-V2
migration; the V1 layout and handle-only V1→V2 migration remain unchanged.

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
  "v20250811",
  "v1.1.0",
  "v1.2.1",
  "v1.3.0",
  "templar-proxy-oracle-near-contract@0.4.1#fb9b3b46bbedd16665dbc31f0efc605c8ef49acfa5ff17608fbc3b490b4a6cb3",
  "templar-proxy-oracle-near-governance-contract@0.3.1#40b1719a525dae7117a776ccefa281f97470e1213e83bbf054bd77eae5b0568a"
]
```

Most market versions are registered under `vX.Y.Z` keys; the legacy `1.0.0` build uses `v20250811`. Proxy oracle and governance builds use `<package>@<version>#<sha256>` keys. The mainnet deployment profile currently deploys new markets from `v1.3.0` and pins the proxy oracle and governance builds shown above; other keys may exist. Released artifact hashes are recorded under [`contract/artifacts/releases/`](https://github.com/Templar-Protocol/contracts/tree/dev/contract/artifacts/releases).

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

`version_key` names the registered version the account was deployed from and `code_hash` is the hash of the bytes deployed. Compare the code hash with the released artifact manifests above, and use [Contract Verification](../addresses.md#contract-verification) to verify the deployed bytes against their source. The record's types are documented in the [API Reference](../api-reference.md).

## Events

The registry logs [NEP-297](https://github.com/near/NEPs/blob/master/neps/nep-0297.md) events under the `templar-registry` standard, version `1.0.0`. Each is logged by the receipt that makes its change, so an asynchronous step that later fails (a global contract that turns out not to exist, a deployment whose account creation fails) logs nothing. As with any NEAR event, count one only if its receipt succeeded: a receipt can still fail after logging, for example when the registry cannot stake the storage a new version needs, and NEAR keeps a failed receipt's logs.

| Event | Fields | Logged when |
| --- | --- | --- |
| `version_added` | `version_key`, `code_hash`, `availability` (`{"Stored":{"code_len":…}}` or `"Global"`) | Uploaded approval or legacy Stored registration stores code, or a callback confirms a global contract exists |
| `version_removed` | `version_key`, `code_hash` | `remove_version` clears a version's stored code |
| `deployed` | `account_id`, `version_key`, `code_hash`, `block_height` | the deployed account is created and initialized and the name resolves to it |
| `code_uploaded` | `code_hash`, `uploader`, `code_len` | code and metadata are persisted and the storage deposit covers their actual cost |
| `upload_removed` | `code_hash`, `uploader` | an unapproved upload is explicitly removed; approval/conversion emits no such event |
| `role_granted` | `account_id`, `role` (`"Uploader"` or `"Deployer"`) | an explicit membership is added, not on duplicate grant |
| `role_revoked` | `account_id`, `role` | an explicit membership is removed, not on absent revoke |

Owner changes are logged by `near-sdk-contract-tools` under its own standard.
