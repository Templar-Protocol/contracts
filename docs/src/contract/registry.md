# Registry

The registry is a contract that maintains a list of contract versions, deploys new contracts, and maintains a list of those deployments. It is the only way a production market or proxy oracle is created: every one is a sub-account of the registry, deployed from a registered, audited version.

The account ID of the mainnet market registry is [`v1.tmplr.near`](https://nearblocks.io/address/v1.tmplr.near). Read `own_get_owner` for its current owner rather than assuming the intended DAO handoff has occurred; what the owner can and cannot do is described in [Protocol Governance](../governance.md#registry-contract).

Role-enabled V2 separates staged uploads from approved versions: Uploader
pre-funds a NEP-145 storage balance and calls nonpayable `upload_code` (Borsh
bytes), Owner approves an Uploaded or Global hash with JSON, and Deployer
creates accounts from approved versions. Owner bypasses action role guards and
alone sets roles, removes version code, upgrades the registry, or uses legacy
`add_version` (including `PublishGlobal`). Query live owner/role/state-version
views before assuming a registry has migrated; see the
[gated rollout](../deployments.md#role-enabled-v2-rollout-alpha-then-v1).

Deployment names `probe` and `deploy` are reserved for implementation-owned
scratch accounts used by Global probing and publication. All authorized callers,
including Owner, receive `Deployment name is reserved`; Deployer authority cannot
occupy either scratch account. Other deployment names retain the normal role checks.

`deploy` accepts `full_access_keys`: Deployer can install keys it controls and
therefore control the child accounts, including replacing their code. Treat this
role as account-creation authority, not merely permission to select approved code.

Only duplicate **pending uploads** are refused. The same bytes may be uploaded
again after approval and registered under multiple version keys; there is no
cross-history approved-hash ban or hash-index bootstrap. Successfully registered
version keys remain occupied permanently, including after code removal. A failed
Global probe preserves a matching upload and frees its version key for retry.

### Storage balances and upload lifecycle

All storage methods are public and independent of roles. Query
`storage_balance_bounds` (`{}`) for `{"min":"<yoctoNEAR>","max":null}` and
`storage_balance_of` (`{"account_id":"<payer>"}`) for the payer's `total` and
`available` balances. The minimum locks exactly the storage for a fixed-size
account ledger record; do not hard-code it. Call payable `storage_deposit` first,
optionally naming `account_id` and `registration_only`. New registration at the
minimum has zero available balance, so uploading also requires a top-up for
actual map/key/uploader/blob storage. Repeated `registration_only:true` on an
already registered account refunds the entire attached deposit.

`upload_code` attaches **zero NEAR** and charges the predecessor's available
balance. Uploaded approval moves the bytes into a version: the original payer
remains charged for retained code until `remove_version` actually removes it.
If the version key and version-storage entry exceed the freed upload metadata,
approval charges the net growth to the original payer; top up that payer's
available balance first or approval fails without changing approval state.
Freed upload metadata and any matching pending storage removed by successful
Global or legacy registration are credited to the original payer. A current
Uploader can call `remove_upload` for its own pending upload; Owner can remove
any. Removal attaches exactly one yocto and credits the original payer's
available balance, not the remover, without immediately transferring funds.
Losing Uploader prevents removal until re-granted (Owner can still act).

`storage_withdraw` requires one yocto and transfers available balance (omit
`amount` to withdraw all available). `storage_unregister` also requires one yocto:
non-force unregister succeeds only once no code/storage charge remains beyond
the registration minimum, then releases that reserve. Forced unregister is
rejected for registered accounts; an unregistered account returns `false`.
There is no code-deleting force option, nonrefundable upload fee, Owner skim, or
new administrative withdrawal method: storage liabilities are user funds.

Solvency checks reserve registry-funded final deployment metadata before dispatch,
and retain that budget until its callback succeeds or fails. They also reserve
the maximum unallocated Owner/proposed-owner slot growth, so standard ownership
methods cannot consume backing for withdrawable balances. A V2 `upgrade` batches
deploy, migration, and private `upgrade_check_storage` in one receipt, including
same-version upgrades: insufficient backing reverts the code change as well.
Target registry releases must retain that private post-upgrade check.
Attach 300 Tgas to `upgrade`: 200 Tgas is reserved for migration and 10 Tgas for
the check, leaving room for decoding and deploying local code.

### Roles, ownership, and migration

`set_role(account_id, role, set, allow_removing_last_member)` is Owner-only and
requires exactly one yocto. `role` is `"Uploader"` or `"Deployer"`, `set`
defaults to `true`, and `allow_removing_last_member` defaults to `false`, following
the RedStone adapter convention. Removing the last explicit member requires
`allow_removing_last_member:true`. `has_role` and
`list_role(role, offset, count)` report **explicit membership only**, even for
Owner; Owner's action-guard bypass is not an implicit membership. `get_upload`
returns only `{uploader, code_len}`, never bytes; `list_uploads` lists hashes with
optional `offset`/`count` pagination.

The standard Owner API, including `own_renounce_owner`, remains available.
Owner plus RBAC preserves existing `own_*` clients, storage, and two-step DAO
handoff; an RBAC Admin-only design is viable but would introduce an unnecessary
ownership API/storage cutover. Renunciation is possible, not prevented by this
design; it removes Owner-only administration unless another authorized recovery
path exists. Ownership transfer preserves explicit grants and moves Owner's
action-guard bypass to the new Owner.

V1→V2 migration moves the historical version/registry map handles without
rewriting entries or Owner slots and initializes empty uploads, version-payer
records, and NEP-145 accounting. Historical code stays registry-funded. Typed
Borsh storage keys preserve the legacy bytes `Registry=0x72` (`r`) and
`Versions=0x76` (`v`), with `Uploads=0x75` (`u`),
`UploadCode=0x63` (`c`) plus hash, `StorageBounds=0x62` (`b`),
`StorageAccount=0x64` (`d`) plus account-ID hash, and
`VersionStorage=0x70` (`p`). The unreleased V2 layout does not imply either live
registry has migrated. A same-V2 code-only upgrade uses empty migration args.

## Interactions

The view examples below use [`near-cli-rs`](../notes.md#contract-interaction-syntax). The outputs are illustrative and abridged; the live call is authoritative.

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

The registry logs [NEP-297](https://github.com/near/NEPs/blob/master/neps/nep-0297.md) events under the `templar-registry` standard. Events are version `1.0.0` except `deployed`, which is version `1.1.0`. Each is logged by the receipt that makes its change, so an asynchronous step that later fails (a global contract that turns out not to exist, a deployment whose account creation fails) logs nothing. As with any NEAR event, count one only if its receipt succeeded: a receipt can still fail after logging, for example when the registry cannot stake the storage a new version needs, and NEAR keeps a failed receipt's logs.

| Event | Fields | Logged when |
| --- | --- | --- |
| `version_added` | `version_key`, `code_hash`, `availability` (`{"Stored":{"code_len":…}}` or `"Global"`) | Uploaded approval or legacy Stored registration stores code, or a callback confirms a global contract exists |
| `version_removed` | `version_key`, `code_hash` | `remove_version` clears a version's stored code |
| `deployed` | `account_id`, `version_key`, `code_hash`, `block_height`, `deployer`, `full_access_keys` | the deployed account is created and initialized and the name resolves to it |
| `code_uploaded` | `code_hash`, `uploader`, `code_len` | code and metadata are persisted and charged to the uploader's available storage balance |
| `upload_removed` | `code_hash`, `uploader` | a pending upload is explicitly removed; approval/conversion emits no such event |
| `role_set` | `account_id`, `role` (`"Uploader"` or `"Deployer"`), `is_granted` (boolean) | every successful `set_role`, including idempotent calls |

For `deployed` 1.1.0, new finalizers record the deploying predecessor and its supplied key list, including `[]` when no keys were supplied. Legacy in-flight finalizers did not carry that metadata, so both fields are `null` for them.

Owner changes are logged by `near-sdk-contract-tools` under its own standard.

The Rust variant is `RegistryEvent::RoleSet { account_id, role, is_granted }`;
for example its NEP-297 data is
`{"account_id":"operator.near","role":"Uploader","is_granted":true}`, under
`templar-registry`, version `1.0.0`.
