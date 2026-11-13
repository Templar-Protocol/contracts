# Deployment specs and state patches

A market is described by one spec file and deployed in two commands. There is
no shell script: the spec is the source of truth, and everything that used to
live in `env.sh`, `market-args.json` and `proxy-*.json` is derived from it.

`deployments/alpha` targets a mainnet registry, so both commands need
`--network mainnet` — the CLI defaults to testnet. `NETWORK` and `SIGNER_ID`
work in place of the flags.

```sh
tmplrmgr market plan  deployments/alpha/<market>.toml --out plan.json \
    --network mainnet --signer-id "$REGISTRY_OWNER" --public-key ed25519:…
tmplrmgr market apply --plan plan.json \
    --network mainnet --signer-id "$REGISTRY_OWNER" --sign-with keychain
```

On role-enabled registries, `registry.deploy` requires the Deployer role or current
Owner; older registries require Owner. A proxy spec additionally requires the
signer to equal `governance.admin`, which the mainnet profiles set to the registry
itself. A role grant does not override that separate governance requirement.

`plan` reads prices and chain configuration and writes a file; it sends nothing
and takes no signing credential. Provider reads may need API credentials.
`apply` sends what the file says.

## Why two steps

The plan is a reviewable artifact. It lists every transaction, decodes the
market configuration for reading, names the keys each new account will grant,
and carries the results of every preflight check. Reviewing a deployment no
longer means reading a shell script and trusting it matches four JSON files.

It is a record of a derivation, not an input: the file carries the spec it came
from, and `apply` re-derives the steps and refuses anything that does not match.
Editing the plan is therefore not a way to change a deployment — change the spec
and re-plan. For something no spec can express, run the transaction yourself
with the command that performs it (`registry deploy`, `proxy-oracle governance
create-proposal`, `storage deposit`); each is typed and validated on its own.

## Writing a spec

Shared values live in `deployments/profiles/`. A market file names the profiles
it extends and states only what differs. Abbreviated — a market also needs a
`[borrow]` leg and the `[market]` parameters the profiles above do not set; see
any file under `deployments/` for a complete one:

```toml
extends = ["../profiles/alpha.toml", "../profiles/irs-standard.toml"]
name = "my-market"

[oracle.direct]                    # reads an oracle that already exists
account_id = "pyth-oracle.near"

[collateral]
asset = "nep141:usdc.near"
price_id = "eaa020c6…"             # the oracle's own identifier
decimals = 6
```

Omit `[oracle.direct]` to deploy a dedicated proxy oracle instead. A proxy
market names `sources` per asset and the deployment creates a governance
contract, the oracle it owns, and the market — seven transactions, plus one
storage registration per NEP-141 asset, rather than one.

### Amounts

Every amount states its unit, and the tool does the scaling:

```toml
[market]
borrow_range            = { minimum = "1 atom" }
supply_range            = { minimum = "0.04 tokens" }
supply_withdrawal_range = { minimum = "0.04 tokens", maximum = "1000 tokens" }
origination_fee         = { Flat = "0 atoms" }
```

`tokens` counts whole units of the borrow asset, scaled by its `decimals` when
the plan is built — `"0.04 tokens"` is four cents of a stablecoin whether it
carries 6 decimals or 7. `atoms` counts the indivisible base units the chain
stores, so `"1 atom"` says "no real floor" in a way `"0.0000001 tokens"` does
not. All three ranges and both fees are denominated in the *borrow* asset;
nothing is stated in collateral.

The unit is mandatory. A bare number is refused rather than guessed at, as is a
`tokens` value with more decimal places than the asset can hold, or a fractional
`atom`. Both spellings parse (`1 atom`, `1 atoms`); the tool writes the plural.

This is what schema 5 changed. A schema 4 file wrote the same amounts as bare
base-unit integers, which are still well-formed numbers — read as whole units
they would be `10^decimals` too large — so such a file is refused by version and
must be re-authored, not renumbered.

## Checking before and after

```sh
tmplrmgr spec check   deployments/alpha/<market>.toml --network mainnet
tmplrmgr market verify <account-id> --network mainnet \
    --governance-admin <account-id> \
    --against deployments/alpha/<market>.toml
```

Both modes verify. A direct market reconstructs without proxies or governance,
so the two governance checks are skipped and everything else runs;
`--governance-admin` is still required and means nothing there.

`verify` re-runs the preflight against what is actually on chain and exits
non-zero on failure, so it can run on a schedule. That matters because the
governance call that configures a price feed is dispatched detached: it reports
success even when the oracle rejected the proxy, so deployed state is the only
witness that a market can price anything.

### Price inputs

`spec check`, `market plan`, `market apply` and `market verify` default to
`--prices-from provider`. For Lazer and RedStone sources, the gateway fetches
signed provider data and asks the deployed adapter to verify it without writing.
The preflight judges whether those selected inputs would produce usable prices
on an on-demand push. Success is **not** proof that the prices currently stored
on chain are usable, and neither planning nor applying a deployment pushes prices.

Only source kinds used by the spec are constructed:

- Lazer needs `--pyth-lazer-api-key` or `PYTH_LAZER_API_KEY`. Its websocket,
  channel and payload-age options are available on these four commands.
- RedStone needs `--redstone-api-key` or `REDSTONE_API_KEY` for RedStone's
  authenticated oracle gateway, and Node.js on `PATH` or an executable selected
  with `--redstone-node-path` / `REDSTONE_NODE_PATH`. A RedStone-only spec needs
  no Lazer key, and a Lazer-only spec needs no RedStone key.
- Pyth and LST sources always read chain state. Direct markets keep their
  existing chain-only oracle checks. `spec check --offline` constructs no providers.

Use `--prices-from chain` to judge stored adapter prices instead. Provider
construction, fetch or verification failures are failed checks; they never
fall back to stored values. `oracle.price.*` and `oracle.aggregate.*` retain
their IDs and name provider, chain or mixed inputs in their details.

Both modes additionally report `oracle.stored.{side}.{i}` for every configured
source. A stored price within the asset's `max_age` (or the market's
`price_maximum_age`) and clock-drift bound passes; stale, missing, unprojectable,
future-drifted or unreadable storage warns. These diagnostics never gate the
command. Chain mode can still fail independently when its selected stored
prices cannot aggregate.

`apply` uses this invocation's mode and provider configuration when rerunning
preflight, not the plan's historical inputs. Its default remains provider even
for a plan written with chain mode. Source options and credentials are not
persisted in specs, plans or journals.

Market plan schema remains 2, but plans containing `"status": "warned"` require
a binary that understands that status; older binaries reject them.

### Reading the report

Checks are printed to stderr as they run, grouped by what is being read, then
summarized. The summary leads with FAILED checks, then WARNINGS, then SKIPPED.
Warnings are non-gating diagnostics, not proofs of success. Warnings and skips
are counted separately from passes; a skipped check proves nothing.

```
→ registry versions
  ok   registry.version.market          v1.3.0
  FAIL registry.version.oracle          `0.5.9` is not registered in v1.tmplr.near; the depl…

5 check(s): 3 passed, 0 warned, 1 skipped, 1 FAILED

FAILED
  registry.version.oracle
    `0.5.9` is not registered in v1.tmplr.near; the deploy would fail partway
```

Colour is used only on a terminal, and `NO_COLOR` turns it off. `-q` silences
the per-check stream but retains the digest. stdout stays the machine-readable
channel throughout, so `spec check … >/dev/null` leaves the report alone and
`… 2>/dev/null | jq` leaves the JSON alone.

`--skip-check <id>` suppresses one verdict — every other check still runs, and
the report records what the skip suppressed, so an override stays reviewable
rather than reading as a pass. An id that matches no check is an error, since a
typo would otherwise silently suppress nothing. Available on `spec check`,
`market plan` and `market apply`.

## Resuming

`apply` journals each step beside the plan as it lands. If a run is
interrupted, re-running it skips what completed and continues from the first
incomplete step. A plan truncated to its completed prefix is refused rather than
reported complete: the re-derivation runs before the journal is consulted.

## Patching contract storage

`tmplrmgr patch` builds one atomic transaction for a contract whose full-access
key is still held: deploy the pinned PatchState WASM, apply guarded storage
operations, then restore the exact local code or global-contract linkage.

### Generate a market configuration patch

```sh
tmplrmgr market generate-patch config "$MARKET" \
  --changes changes.json --out config-patch.toml --network testnet
```

This command reads finalized chain state and writes local artifacts only. It has no
signer or submission options. `--changes` names a JSON file; `-` reads stdin.
The generic configuration editor accepts ordered RFC 6902 `test` and `replace`
operations, including nested JSON Pointers. For example:

```json
[
  {"op":"test","path":"/borrow_asset","value":{"Nep141":"old-token.testnet"}},
  {"op":"replace","path":"/borrow_asset","value":{"Nep141":"new-token.testnet"}}
]
```

NEP-245 configuration assets use
`{"Nep245":{"contract_id":"tokens.testnet","token_id":"chain:asset:id"}}`.
These are market configuration values, not gateway `TokenReference` values.
One changes file can replace both asset fields or edit another configuration field.

Decimal-valued fields accept ordinary decimal strings in both `test` and `replace`
operations: `"1.4"` compares by the same fixed-point value as its canonical
serialization. Other scalars retain their exact JSON representation; unknown
fields and semantic no-ops are rejected.

Only the exact released Market 1.6.1 WASM is initially admitted. The layout is backed
by the real deployed-state capture in `tools/manager/fixtures/market-layouts/`;
catalog membership, an account name, or a successful view alone is insufficient.
Unknown/current-source code requires separate layout evidence before admission.
Generation cross-checks the decoded configuration against a view at the same block,
preserves every opaque byte outside its Borsh span, and guards the entire old `STATE`.
The capture also pins the configuration's Borsh schema fingerprint so changes to
the current Rust layout require reviewing the released-layout evidence. Validation
uses current protocol rules; the fingerprint does not freeze the 1.6.1 validator.

The output is a schema-3 patch spec plus `<stem>.blobs/` containing old/new `STATE`,
canonical before/after JSON, and `report.json`. The report binds the snapshot,
code/restore identity, byte offsets, hashes, changes, and economic caveats.
The spec pins the effective code hash, the entire old `STATE` hash, and the
replacement file's SHA-256. Planning, replay, and apply reject code drift or
modified replacement bytes, even when storage is unchanged.
Existing outputs are never overwritten. Use the ordinary plan/replay/apply workflow below.

Actual edits to `time_chunk_configuration` or `borrow_interest_rate_strategy` are
rejected because they require migration of persisted derived interest state.
Other valid edits are not certified economically safe: asset changes do not convert
inventory or establish raw-unit equivalence; MCR, fees, ranges, duration, oracle,
yield-weight, and recipient changes can affect existing positions and history.
Neither zero in-flight counters nor an oracle pause establish market isolation.
State drift requires a fresh reviewed plan.

### Author and execute a storage patch

Export complete, block-pinned contract storage before authoring guards:

```sh
tmplrmgr patch export <account> --out deployments/patches/<account>/<date>-<slug>.toml \
  --network mainnet
```

The export writes every contract-storage entry as schema-3 TOML and a sibling
`<stem>.blobs/` directory. Values are stored as deterministic files named from
the SHA-256 of their raw keys. Planning separately re-fetches the pinned account
metadata, access keys, code, and contract linkage. Existing spec or blob paths
are never overwritten. A `patch.state_complete` check reports whether the trie
fit in one request or required widening one-byte prefixes; incomplete or
conservatively unaccountable state aborts without creating output.

Review and build the plan:

```sh
tmplrmgr patch plan deployments/patches/<account>/<date>-<slug>.toml \
  --out patch-plan.json --network mainnet \
  --signer-id <account> --public-key ed25519:…
tmplrmgr patch dry-run --plan patch-plan.json --network mainnet
```

`patch dry-run` reconstructs the target under its literal account ID in a fresh
sandbox, installs the fetched code and complete state, and executes the exact
reviewed `tx.batch`. Each view `[[check]]` is called before and after. An
`expect` or `expect_json` compares only the after JSON; a check without either
passes after a successful call and still reports its observed value. Before
failures and before/after differences are diagnostic; after failures and
expectation mismatches fail the check. JSON Patch diffs are present when both
calls return JSON. Use `expect_json = '{"maximum":null}'` for an exact JSON
expectation containing nulls, which TOML cannot represent natively. It contains
validated JSON text and is mutually exclusive with `expect`; existing schema-3
specs and plan encodings are unchanged when this optional field is absent.

A `[[check]]` with `code_hash = "<base58 hash>"` requires that effective contract
code before planning, replay, and apply. A byte expression
`{ pinned_file = { path = "new-state.bin", sha256 = "<64 hex digits>" } }`
reads the file relative to the spec and verifies its bytes before resolving the
patch. These additive schema-3 forms require a manager that understands them;
existing encodings and plan digests are unchanged when they are absent.

The dry-run prints one machine-readable JSON report on stdout, including the
transaction, target code hash, every before/after view result, JSON diff, and
check verdict. Reporter output, progress, diagnostics, and the digest go to
stderr. Keep stdout dedicated to the report when piping it to review tooling.
The completed replay is stamped into the same plan when no replay check fails.
An apply-valid stamp requires every replay check to have passed; apply rejects
failed, warned or skipped proof checks. The stamp binds the plan digest,
semantic complete-state digest, target code hash, and verdicts; it records the
sandbox chain ID for review context.

Apply only after reviewing both the plan and stamped replay:

```sh
tmplrmgr patch apply --plan patch-plan.json \
  --network mainnet --signer-id <account> --sign-with keychain
```

Apply re-derives the spec, live code/linkage, complete-state digest, and batch;
state drift invalidates the plan. Without an explicit override it refuses a
missing, stale, digest-mismatched, or failed `patch.dry_run` stamp.
`--skip-check patch.dry_run` and `--skip-check patch.state_complete` are
rejected. If local replay is unavailable and the operator accepts that risk,
use the dedicated override:

```sh
tmplrmgr patch apply --plan patch-plan.json --no-dry-run \
  --network mainnet --signer-id <account> --sign-with keychain
```

`--no-dry-run` records `patch.dry_run` as explicitly skipped; all other
preflight checks still run. Prefix deletes are expanded from one verified full
snapshot and retain an in-receipt expectation for every concrete removal.
Accounts containing record kinds the accounting reader cannot enumerate are
rejected rather than silently treated as complete.

The plan is not self-contained: apply re-reads its canonical source spec and
referenced files at their original paths. Dry-run views are sandbox evidence;
check the live account separately after apply.

Schema-2 storage-only specs are not accepted. Review the authored operations
and checks, set `schema = 3`, then rerun `tmplrmgr patch plan`; alternatively,
export a fresh schema-3 spec with the command above.

Authored `set` and single-key `remove` operations should state `expect`. Use
`expect = "absent"` for a fresh key; it compiles to an in-receipt absence guard.
Keys and values use `utf8`, `hex`, `base64`, `file`, `concat`, `sha256`, `json`,
or `borsh` byte expressions. `file` is relative to the declaring spec.

This is a privileged authorization checklist:

- Confirm the target account, full-access signer, and plan public key.
- Inspect the released PatchState 0.1.0 artifact and pinned SHA-256.
- Confirm the batch receiver, spec target, and PatchState payload account match.
- Inspect all view before/after values and diffs, including no-expect checks.
- Verify the apply-time stamp binding and resolved-state re-derivation checks.
- Authorize only after the complete arbitrary-storage write is understood.

## Registering a contract release

Prefer a canonical published release when adding a registry version:

```sh
tmplrmgr registry add-version --registry-id v1.tmplr.near \
  --contract market --release 1.6.1 --deploy-mode global-hash \
  --network mainnet --signer-id tmplr.near --print json
```

This only prints a transaction plan. To submit it with the registry owner's key,
replace `--print json` with `--sign-with keychain`; confirm the current owner first.
`global-hash` publishes the code globally and registers its hash, whereas `normal`
stores the code in the registry. The command estimates the required deposit.

`--release` selects an exact version from `contract/artifacts/releases/` compiled
into the manager. The asset is downloaded from its recorded GitHub Release (or
read from the shared cache) and verified against the catalog's byte length and
SHA-256. The version key is derived automatically as
`{package}@{version}#{sha256}` and cannot be overridden for a release.
An uncatalogued release fails; it never falls back to a local build.
Rebuild the manager after a release's catalog PR merges to make it available.

For deliberate unpublished-code registration, use `--contract market --build`
instead of `--release`: this runs a reproducible build from the selected workspace
and derives the key from its package metadata and built bytes.
`--package <Cargo-package-or-artifact-ID>` (alias `--artifact`) supports the same
release/build selection. Both selectors require exactly one of `--release` or
`--build`. The old `--no-build` path has been removed; existing local build output
is not used implicitly.

The explicit escape hatches `--wasm <path>` and `--code-hash <base58-hash>` still
require `--version-key`. Neither accepts `--release` or `--build`; `--code-hash`
registers an already-published global contract and does not take `--deploy-mode`.

## Upgrading a registry

The observed mainnet `v1.tmplr.near` baseline is registry release **3.1.0** with
stored state **V1**, not release 2.0.0. Re-read each target's deployed metadata and
state before acting; software release numbers and state versions are distinct.
The Owner-only `upgrade` deploys code and runs `migrate` atomically in the same
receipt, so a failed migration reverts the deploy. A successful upgrade has no
automatic rollback.

```sh
tmplrmgr registry upgrade --registry-id <registry> \
  --network mainnet --signer-id <owner> --sign-with keychain
```

`--release <version>` picks a catalogued registry release; the newest is the default.
`--migrate-args-file <path>` passes the release's `state::Migration` chain to `migrate`; omit it
when the stored state version is unchanged. The command submits nothing unless every check passes,
and none can be skipped:

- the deployed code hashes to a catalogued registry release whose NEP-330 version agrees;
- the target WASM is a catalogued release no older than the deployed one, verified against its
  pinned sha256;
- the signer is the registry's owner, holding a full-access key, and the registry has the balance
  to stake the new code;
- the exact planned transaction is replayed against the registry's complete, block-pinned state in
  a fresh sandbox. The replay must reproduce what mainnet serves, plan the same transaction,
  succeed within half the attached gas, and leave the registry on the new code at a current state
  version with its owner, versions (code hashes and availability) and deployments unchanged.
  Without a migration its storage may move only by the code swap — the views cannot see a lost
  blob or reserved name, storage can;
- no name is still reserved by an unfinished deploy, whose finalize callback would otherwise run
  against the new code;
- the account — code, keys and every storage entry, including writes not yet final — is byte for
  byte what was snapshotted, re-read just before submitting, with its balance no lower; and the
  signer plans the replayed transaction.

After submitting, the same post-conditions are checked on chain, with storage required to equal
exactly what the replay measured. If the submission's outcome never comes back, nothing is
verified: the transaction may still land, so read the registry's code hash and state version before
doing anything else. `--print json --public-key <key>` runs every check against the key that will
sign and prints the planned transaction instead of signing it; the checks hold only for the state
they read, so sign it straight away or re-run. Like `patch export`, the snapshot needs an RPC that
pages `view_state` past the stock 50 kB limit, such as FastNEAR's
(`--rpc-url https://rpc.mainnet.fastnear.com`).

### Role-enabled V2 rollout: alpha, then v1

This is an operator procedure, **not authorization to publish, transact, vote, or
delete keys**. Complete the entire sequence on `templar-alpha.near` before
starting it on `v1.tmplr.near`. Use a reviewed, tagged registry release whose
immutable WASM is published and catalogued; require the fast, sandbox, Wasm,
lint, and DAO/keyless runtime checks to have passed for that release.

1. **Record the baseline and approve the operators.** Set `REGISTRY` to the
   current canary/production target, `RELEASE` to the audited version, and
   `CURRENT_OWNER` from `own_get_owner`, not from an example. Record finalized
   local/global code identity, stored/target state versions, `needs_migration`,
   owner/proposed owner, every version and deployment/registry entry, balance,
   and access keys. Page through the lists; `get_registry_entry` distinguishes
   Reserved from Deployed. Require no in-flight deployment or global
   registration, including provisional Global entries whose callbacks have
   not finalized. Resolve the release's catalog SHA-256 and verified bytes.
   Before upgrade or global publication, also verify that `probe.$REGISTRY` and
   `deploy.$REGISTRY` do not already exist as child accounts and have no blocking
   registry entries. These implementation-owned labels are now reserved from
   deployment for Owner and every Deployer; reservation does not evict historical
   children. Halt and reconcile any blocker rather than assuming the new guard
   repairs it. This is a required live check, not a claim about either target.
   The intended DAO is `templar.sputnik-dao.near`: verify its deployed policy,
   proposal bond, council and operator approval before any proposal. Obtain
   real `UPLOADER` and `DEPLOYER` account IDs, confirm they exist and have usable
   signing keys, and fund the registry for owner-funded metadata/role storage.
   Never infer credentials or copy an example operator.

2. **Upgrade while the verified current owner can still sign.** For stored V1,
   `MIGRATION_JSON` must contain exactly `{"from_version":"v1"}`:

   ```sh
   tmplrmgr registry upgrade --registry-id "$REGISTRY" --release "$RELEASE" \
     --migrate-args-file "$MIGRATION_JSON" --network mainnet \
     --signer-id "$CURRENT_OWNER" --sign-with keychain
   ```

   For already-current V2 replacing code only, omit `--migrate-args-file`.
   Halt on any other stored layout; do not invent migration arguments. Keep the
   manager's replay gate intact. Verify the catalog code identity, stored and
   target version 2, `needs_migration=false`, unchanged owner, historical code
   bytes/availability and deployment entries before proceeding.
   A V2-origin upgrade also runs private `upgrade_check_storage` after migration
   in the same atomic receipt, including when migration arguments are empty.
   The target release must expose this private check; it prevents newly staked
   local code from spending users' available storage balances.
   The V1 migration preserves typed Borsh key bytes for the historical registry
   (`0x72`, `r`) and versions (`0x76`, `v`) and moves their map handles without
   rewriting entries or Owner slots. New V2 keys are bounds `0x62` (`b`), upload
   code `0x63` (`c`) plus hash, storage account `0x64` (`d`) plus account-ID hash,
   version-payer records `0x70` (`p`), and uploads `0x75` (`u`).
   Historical code remains registry-funded; there is no history hash-index
   bootstrap. These are target-layout requirements, not claims of live V2.

3. **Grant hot-key roles and publish the upgrade artifact before handoff.**
   Current Owner calls `set_role` with
   `{"account_id":"<verified uploader>","role":"Uploader","set":true}` and
   `{"account_id":"<verified deployer>","role":"Deployer","set":true}`, one yocto each.
   The generic JSON transaction syntax is:

   ```sh
   near contract call-function as-transaction "$REGISTRY" set_role \
     json-args "$ROLE_JSON" prepaid-gas '30 Tgas' attached-deposit '1 yoctoNEAR' \
     sign-as "$CURRENT_OWNER" network-config mainnet sign-with-keychain send
   ```

   Verify `has_role` and `list_role` for both operators; both report explicit
   membership only. Owner's action bypass does not synthesize a membership.
   Omitted `set` defaults to true; `allow_removing_last_member` defaults to false.
   Revocation uses `set:false`, with `allow_removing_last_member:true` required
   when removing the last explicit member. Every successful setter emits
   `role_set` with `account_id`, `role`, and `is_granted`, including idempotence.
   Publish the exact verified registry WASM using retained Owner-only
   `PublishGlobal`, under a reviewed unused `REGISTRY_GLOBAL_KEY`:

   ```sh
   tmplrmgr registry add-version --registry-id "$REGISTRY" \
     --wasm "$REGISTRY_WASM" --version-key "$REGISTRY_GLOBAL_KEY" \
     --deploy-mode global-hash --network mainnet \
     --signer-id "$CURRENT_OWNER" --sign-with keychain
   ```

   Confirm all receipts succeed, `get_version` reports Global, and its base58
   SHA-256 equals the audited release's bytes. Save that hash as
   `REGISTRY_GLOBAL_HASH`. An already-finalized matching Global registration
   may be reused; do not overwrite a key or treat a pending probe as approval.

4. **Transfer ownership through the DAO itself.** The verified `CURRENT_OWNER`
   from step 1 calls `own_propose_owner` with
   `{"account_id":"templar.sputnik-dao.near"}`, one yocto. Submit a real DAO
   FunctionCall proposal containing this action:

   ```json
   {
     "FunctionCall": {
       "receiver_id": "<REGISTRY>",
       "actions": [{
         "method_name": "own_accept_owner",
         "args": "e30=",
         "deposit": "1",
         "gas": "30000000000000"
       }]
     }
   }
   ```

   `<REGISTRY>` is the verified current target. Wrap the kind in
   `{"proposal":{"description":"<reviewed rationale>","kind":<kind above>}}`
   for `add_proposal`, attaching the DAO's **actual** proposal bond. Follow its
   normal review/vote process; when voting via `act_proposal`, echo the exact
   stored proposal kind. Record the proposal ID, votes, final status and every
   execution receipt. Require actual `own_get_owner=templar.sputnik-dao.near`
   and a cleared proposed owner. A proposal ID or vote alone proves nothing;
   never sign a direct acceptance pretending to be the DAO. Standard
   `own_renounce_owner` remains exported; do not renounce during handoff.

5. **Prove approval, deployment, and same-code upgrade before deleting keys.**
   First query the live storage minimum and uploader balance:

   ```sh
   near contract call-function as-read-only "$REGISTRY" storage_balance_bounds \
     json-args '{}' network-config mainnet now
   near contract call-function as-read-only "$REGISTRY" storage_balance_of \
     json-args "{\"account_id\":\"$UPLOADER\"}" network-config mainnet now
   ```

   Bounds return yoctoNEAR strings, with `max:null`. Registration locks the
   measured fixed-size account-record minimum; depositing only that minimum
   leaves zero available for uploads. Set `STORAGE_DEPOSIT` to an amount in
   NEAR/yoctoNEAR units covering registration (if needed) plus actual
   map/key/uploader/blob storage and a reviewed surplus. Prefund the uploader:

   ```sh
   near contract call-function as-transaction "$REGISTRY" storage_deposit \
     json-args '{"registration_only":false}' prepaid-gas '30 Tgas' \
     attached-deposit "$STORAGE_DEPOSIT" sign-as "$UPLOADER" \
     network-config mainnet sign-with-keychain send
   ```

   Omitted `account_id` credits the predecessor. Re-read `storage_balance_of`
   and require sufficient available balance. `registration_only:true` on an
   already registered target refunds the entire deposit, so do not use it for a
   top-up. These storage methods are unrestricted by roles.

   Uploader sends reviewed artifact bytes as Borsh `Vec<u8>` (four-byte
   little-endian length followed by the bytes) to **nonpayable** `upload_code`.
   Prepare and inspect `UPLOAD_BORSH` from the verified artifact, including the
   length prefix, not the bare WASM:

   ```sh
   near contract call-function as-transaction "$REGISTRY" upload_code \
     file-args "$UPLOAD_BORSH" prepaid-gas '100 Tgas' \
     attached-deposit '0 NEAR' sign-as "$UPLOADER" \
     network-config mainnet sign-with-keychain send
   ```

   Confirm the returned base58 SHA-256 against the audited bytes,
   `get_upload`'s uploader/length, and the storage balance charge. Only duplicate
   pending uploads are rejected: already-approved bytes may be uploaded again
   and approved under another version key. Empty/non-WASM uploads are permitted,
   not evidence of deployability.

   For Uploaded approval, compare the planned key/version-entry growth with the
   freed upload metadata and top up the original payer for any net increase.
   The DAO's one-yocto deposit is not uploader storage funding; insufficient
   available balance rejects approval.

   Through the DAO proposal envelope above, execute `approve_version` with
   UTF-8 JSON `{"version_key":"<reviewed key>","code":{"Uploaded":"<upload hash>"}}`,
   base64-encoded into the action's `args`, deposit `"1"` and gas
   `"80000000000000"`. Alternatively `{"Global":"<published hash>"}` probes
   existing global code. Require successful execution receipts and VersionAdded,
   exact version hash/availability, and consumed upload. Failed probes free the
   key and preserve uploads; successful keys remain occupied even after
   `remove_version`. Uploaded approval keeps the original uploader charged for
   retained code until `remove_version` removes it; freed pending metadata is
   credited to that payer's available balance. Successful Global or legacy
   matching-upload cleanup likewise credits freed pending storage. Explicit
   `remove_upload` attaches one yocto and credits the original payer, never the
   remover, without an immediate transfer. `storage_withdraw` attaches one yocto
   and withdraws available funds; omitted `amount` withdraws all available.
   `storage_unregister` attaches one yocto and can release the registration
   reserve only after all other locked charges are gone. Forced unregister is
   rejected for registered accounts (an unregistered target returns false).
   There is no nonrefundable upload fee, Owner skim, or administrative withdrawal
   method: storage liabilities remain user funds.

   Have the separate Deployer deploy the approved artifact; verify child code
   identity, initialization, deployment record and the explicitly reviewed
   `full_access_keys`. Require the `Deployed` 1.1.0 event's `deployer` and
   `full_access_keys` to match that call; provenance is null only for legacy
   in-flight finalizers. Deployer can install its own keys and thereby control
   the child, including replacing code: this trust boundary is not limited to
   the approved version. During normal rotation, confirm the child works,
   install the freshly derived MPC/DAO-controlled key, have the DAO delete the
   deployer key with the new key, then list the child's keys and require only
   the MPC/DAO-controlled key to remain.

   Then submit a DAO `upgrade` action whose decoded UTF-8 JSON is
   `{"code":{"GlobalHash":"<REGISTRY_GLOBAL_HASH>"},"migrate_args":""}`, deposit
   `"1"` and gas `"280000000000000"`. The voting transaction needs 300 Tgas;
   verify that the live DAO's execution/callback policy supports this allocation.
   Require all receipts to succeed, effective GlobalHash equal to the audited
   registry release, current V2, unchanged owner, grants, uploads, versions and
   deployments. The mock DAO tests prove predecessor/callback behavior, not the
   production council's threshold or gas policy.

6. **Delete keys last, then prove operation again.** Only after every preceding
   gate passes, enumerate and delete **every** registry full-access key using
   the registry account's existing authorized signing path:

   ```sh
   tmplrmgr account delete-key --key "$PUBLIC_KEY" --network mainnet \
     --signer-id "$REGISTRY" --sign-with keychain
   ```

   This is a registry-account transaction, not an Owner contract call. Review
   remaining function-call keys for forbidden authority; record deletion
   receipts and the final key list. With no registry signing key, repeat the DAO
   same-code upgrade and a distinct Uploader→DAO approval→Deployer deployment.
   Verify successful receipts and preserved state again before promoting alpha
   to v1. Key deletion is irreversible unless another authorized path remains.

If any proposal fails or its outcome is unknown, inspect finalized DAO and
registry state before retrying; never remove keys with incomplete evidence.
The manager upgrade command requires an owner full-access signer and cannot
impersonate a DAO after handoff: subsequent upgrades are direct DAO proposals.
Registry-DAO proposal tooling must expose an explicit action-gas override before
live rollout. Gateway/tmplrmgr upload/approval UX and removal of legacy
`PublishGlobal` are separate follow-ups; this rollout does not depend on them.
