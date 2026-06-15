# Templar Soroban Vault CLI

`tmplr-soroban-vault` deploys and operates Templar Soroban vault stacks.

The CLI delegates most transaction construction, signing, and submission to the installed
`stellar` CLI. It owns the Templar-specific pieces around artifact hashing, WASM upload/reuse,
deployment manifests, compact vault command payloads, and operator command routing.

## Deployment

Before deploying, run `doctor` to check local readiness without submitting transactions:

```sh
tmplr-soroban-vault doctor
```

`doctor` checks the installed `stellar` CLI, configured network/passphrase/RPC inputs, source
identity availability without printing secret values, manifest writability, expected WASM artifacts,
Docker mount health when running inside a container, and mainnet guard status.

```sh
cargo run -p templar-soroban-vault-cli -- \
  deploy stack \
  --governance-timelock-ns 86400000000000
```

By default, deployment state is stored in:

```text
contract/vault/soroban/.deploy-state/manifest.json
```

The deploy flow reuses contract IDs already recorded in the manifest unless `--force-new` is set.
It reuses uploaded WASM by local SHA-256 hash when the hash can be fetched from the configured
network, and uploads only when the WASM is missing remotely.
When the CLI builds WASM artifacts, it embeds `source_repo` contract metadata for explorer
build-info and source-attestation discovery. The default is
`github:Templar-Protocol/contracts`; override it with `--contract-source-repo` or
`SOROBAN_CONTRACT_SOURCE_REPO`, or pass an empty value to disable the metadata.
During non-dry-run deployment writes, the manifest is checkpointed after each successful artifact
upload/reuse decision, contract deploy/import record, asset-token record, and initialization. If a
later initialize call fails, rerunning the command can reuse the IDs already written to the manifest.
Use `reconcile` or `deploy repair` to compare a checkpointed manifest with chain state before
manual recovery. The repair plan classifies each component as `missing`, `deployed`, `initialized`,
`unknown`, or `mismatched`, includes fetched on-chain WASM hashes where available, and reports
safe next steps. `deploy resume` runs the same reconciliation gate and continues only when recorded
contracts are not mismatched or unknown.
In interactive human mode, `deploy stack` shows a progress bar across WASM upload/reuse, contract
deployment/reuse, initialization, and adapter deployment stages. Progress rendering is disabled for
`--json`, `--json-lines`, `--dry-run`, and non-TTY stderr.

The vault runtime WASM keeps its embedded contract spec. Vault initialization uses the ordinary
`stellar contract invoke -- initialize ...` path, so the Stellar CLI can resolve the ABI from the
deployed WASM. Configure RPC with `--rpc-url`, `STELLAR_RPC_URL`, or a profile `rpc_url`. Keep
signing material in the Stellar keystore or `STELLAR_ACCOUNT`; the CLI does not require secrets in
argv.

For write commands that emit a transaction hash, the CLI polls `stellar tx fetch` until RPC reports
success or failure, up to 300 seconds. If the original Stellar command exits after emitting a hash
and RPC later reports success, the CLI treats the transaction as confirmed and records the hash.

Pass `--blend-pool` once per Blend pool to deploy one adapter per pool. The manifest stores these
as `blend_adapter_0`, `blend_adapter_1`, and so on. On an existing deployment, new pools are
appended and pools already present in the manifest are left unchanged unless `--force-new` is set.
Pass `--custodian` once per custodian or multisig address to deploy custodial adapters in the same
flow. The manifest stores these as `custodial_adapter_0`, `custodial_adapter_1`, and so on, with
the `admin`, `vault`, `custodian`, and bound `asset` constructor args recorded for reconciliation
and status. Custodial adapters are single-asset; `deploy adapters --custodian ...` requires
`asset_token` in the manifest or `--asset-token`.

```sh
tmplr-soroban-vault deploy stack \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --blend-pool CPOOL2... \
  --custodian G...
```

To add adapters later without redeploying the stack, use `deploy adapters`. If the manifest already
contains `vault` and `governance`, only the requested adapter WASM and new adapter instances are
touched. For imported deployments, pass the existing contract ids once and the CLI records them in
the manifest before appending adapters.

```sh
tmplr-soroban-vault deploy adapters \
  --vault CVAULT... \
  --governance CGOV... \
  --asset-token CASSET... \
  --blend-pool CPOOL... \
  --custodian G...
```

Use `deploy plan` to inspect reuse, upload, deploy, and manifest decisions without network writes or
manifest changes:

```sh
tmplr-soroban-vault deploy plan stack \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --custodian G...

tmplr-soroban-vault deploy plan adapters \
  --vault CVAULT... \
  --governance CGOV... \
  --blend-pool CPOOL... \
  --custodian G...
```

Recover an interrupted deployment by reconciling first, then resuming if the repair plan reports
`safe_to_resume: true`:

```sh
tmplr-soroban-vault reconcile --json
tmplr-soroban-vault reconcile --skip-view-verification --json
tmplr-soroban-vault deploy repair --json
tmplr-soroban-vault deploy resume \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --custodian G...
```

The CLI validates Soroban account and contract addresses at parse time for operational commands.
WASM hashes accepted by governance upgrade commands must be 32-byte hex values.

## Curator Role Models

The CLI uses the contract name `governance admin` for the address that controls the governance
contract, but operationally that address can represent several curator models. It can be a single
operator key, a multisig contract, or a governance contract controlled by an external process. A new
stack uses `--admin` as both the governance admin and the initial vault curator. After deployment,
governance proposals can split that into separate curator, sentinel, and allocator identities.

Common role terms:

- `governance admin`: the address passed as `--admin` on governance commands. It submits and accepts
  governance proposals. For multisig governance, this is the multisig contract address.
- `vault curator`: the runtime curator address. It starts as the deployment `--admin` and can be
  changed with `governance submit-set-curator`.
- `sentinel`: an emergency backstop configured with `governance submit-set-sentinel`. It can only
  take protective actions such as pausing or tightening restrictions; governance admin is still
  required to relax or unpause.
- `allocator`: one or more addresses configured with `governance submit-set-allocators`. Allocators
  run market allocation and refresh operations. The vault curator is always authorized as an
  allocator too.
- `adapter`: a market route such as a Blend adapter or custodial adapter. The governance admin must
  allow adapters and place them into the typed supply queue before allocations can route through
  them.

For timelocked deployments, submit commands create proposals and `accept-ready` accepts them only
after the relevant timelock has elapsed. Use `governance queue` and `governance explain` to inspect
pending proposal ids and readiness.

### Single Curator

Use this model when one operational address controls governance and day-to-day allocation. This is
the simplest testnet or custodial setup.

```sh
tmplr-soroban-vault deploy stack \
  --admin GCURATOR... \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL...

tmplr-soroban-vault governance submit-set-allowed-adapters \
  --admin GCURATOR... \
  --adapters CBLENDADAPTER...
tmplr-soroban-vault governance accept-ready --admin GCURATOR... --kind allowed-adapters

tmplr-soroban-vault governance submit-set-supply-queue \
  --admin GCURATOR... \
  --entry 0:CBLENDADAPTER...
tmplr-soroban-vault governance accept-ready --admin GCURATOR... --kind supply-queue

tmplr-soroban-vault curator allocate-supply \
  --caller GCURATOR... \
  --market 0 \
  --amount 1.25 \
  --asset-decimals 7
```

For zero-timelock local deployments, the curator convenience commands can submit and immediately
accept adapter setup:

```sh
tmplr-soroban-vault curator set-allowed-adapters \
  --admin GCURATOR... \
  --adapters CBLENDADAPTER... \
  --auto-accept
tmplr-soroban-vault curator set-supply-queue \
  --admin GCURATOR... \
  --entry 0:CBLENDADAPTER... \
  --auto-accept
```

### Multisig Governance Curator

Use this model when the curator is decentralized and governance actions are controlled by a multisig
or governance contract. Deploy with the multisig as `--admin`; the CLI treats that address as the
governance caller, while the Stellar source account supplies the transaction envelope.

```sh
tmplr-soroban-vault deploy stack \
  --admin CMULTISIG... \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL...

tmplr-soroban-vault governance plan-submit-set-supply-queue \
  --admin CMULTISIG... \
  --entry 0:CBLENDADAPTER...
```

When the multisig can authorize the child invocation directly, submit and accept through the CLI:

```sh
tmplr-soroban-vault governance submit-set-allowed-adapters \
  --admin CMULTISIG... \
  --adapters CBLENDADAPTER...
tmplr-soroban-vault governance queue --kind allowed-adapters
tmplr-soroban-vault governance accept-ready --admin CMULTISIG... --kind allowed-adapters
```

If the multisig requires a separate proposal flow, use the `plan-*`, `queue`, `explain`, and
`pending` commands to prepare and audit the intended action, then submit the equivalent invocation
through the multisig workflow.

### Curator With Sentinel

Use this model when a separate emergency key or contract can pause the vault or tighten transfer
restrictions, while normal governance remains with the governance admin.

```sh
tmplr-soroban-vault governance submit-set-sentinel \
  --admin GCURATOR_OR_MULTISIG... \
  --sentinel GSENTINEL...
tmplr-soroban-vault governance accept-ready --admin GCURATOR_OR_MULTISIG... --kind sentinel
```

Governance-admin pause and restriction changes use the normal timelocked proposal path:

```sh
tmplr-soroban-vault governance submit-set-paused \
  --admin GCURATOR_OR_MULTISIG... \
  --paused true
tmplr-soroban-vault governance submit-set-restrictions \
  --admin GCURATOR_OR_MULTISIG... \
  --mode blacklist \
  --accounts GACCOUNT...
```

Sentinel emergency actions are immediate governance-contract entrypoints. They bypass the proposal
queue and are intentionally one-way: the sentinel can pause or tighten restrictions, but cannot
unpause or relax restrictions. Use `stellar contract invoke` with the same network/profile
environment as the vault CLI:

```sh
stellar contract invoke \
  --id "$SOROBAN_GOVERNANCE" \
  --source-account sentinel \
  -- set_paused \
  --caller GSENTINEL... \
  --paused true

stellar contract invoke \
  --id "$SOROBAN_GOVERNANCE" \
  --source-account sentinel \
  -- set_restrictions \
  --caller GSENTINEL... \
  --mode 1 \
  --accounts '["GACCOUNT..."]'
```

The governance admin restores normal operation:

```sh
tmplr-soroban-vault governance submit-set-paused \
  --admin GCURATOR_OR_MULTISIG... \
  --paused false
tmplr-soroban-vault governance accept-ready --admin GCURATOR_OR_MULTISIG... --kind pause
```

### Curator With Allocators

Use this model when governance is retained by the curator or multisig, but allocation execution is
delegated to one or more hot or automated addresses. Configure allocators through governance, then
use the allocator address as the `--caller` for market operations.

```sh
tmplr-soroban-vault governance submit-set-allocators \
  --admin GCURATOR_OR_MULTISIG... \
  --allocators GALLOCATOR1...,GALLOCATOR2...
tmplr-soroban-vault governance accept-ready --admin GCURATOR_OR_MULTISIG... --kind allocators

tmplr-soroban-vault curator refresh-markets \
  --caller GALLOCATOR1... \
  --markets 0,1
tmplr-soroban-vault curator allocate-supply \
  --caller GALLOCATOR1... \
  --market 0 \
  --amount 100 \
  --asset-decimals 7
tmplr-soroban-vault curator allocate-withdraw \
  --caller GALLOCATOR1... \
  --market 0 \
  --amount 25 \
  --asset-decimals 7
```

### Curator With Sentinel And Allocators

Use this model for a more separated production setup: governance admin or multisig controls policy,
allocator keys execute routine market operations, and a sentinel key or contract handles emergency
pause/restriction tightening.

```sh
tmplr-soroban-vault deploy stack \
  --admin GCURATOR_OR_MULTISIG... \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --custodian GCUSTODIAN...

tmplr-soroban-vault governance submit-set-sentinel \
  --admin GCURATOR_OR_MULTISIG... \
  --sentinel GSENTINEL...
tmplr-soroban-vault governance submit-set-allocators \
  --admin GCURATOR_OR_MULTISIG... \
  --allocators GALLOCATOR1...,GALLOCATOR2...
tmplr-soroban-vault governance submit-set-allowed-adapters \
  --admin GCURATOR_OR_MULTISIG... \
  --adapters CBLENDADAPTER...,CCUSTODIALADAPTER...
tmplr-soroban-vault governance submit-set-supply-queue \
  --admin GCURATOR_OR_MULTISIG... \
  --entry 0:CBLENDADAPTER... \
  --entry 1:CCUSTODIALADAPTER...

tmplr-soroban-vault governance queue
tmplr-soroban-vault governance accept-ready --admin GCURATOR_OR_MULTISIG...
```

After setup, allocators operate the supply queue and sentinel handles emergencies:

```sh
tmplr-soroban-vault curator allocate-supply \
  --caller GALLOCATOR1... \
  --market 1 \
  --amount 1000 \
  --asset-decimals 7

stellar contract invoke \
  --id "$SOROBAN_GOVERNANCE" \
  --source-account sentinel \
  -- set_paused \
  --caller GSENTINEL... \
  --paused true
```

For custodial adapters, use `deploy adapters --custodian <address>` to append custodial routes,
then allow the deployed adapter and add it to the supply queue before allocating to it. Each
custodial adapter is bound to the manifest asset token at deployment and rejects calls for any
other asset. The custodian, adapter admin, or vault can explicitly report route NAV on the adapter.
Reports include the current stored NAV and a monotonically increasing nonce so stale heartbeats
cannot re-add assets that have already been released back to the vault:

```sh
stellar contract invoke \
  --id "$CUSTODIAL_ADAPTER_ID" \
  --source-account custodian \
  -- set_reported_assets \
  --caller GCUSTODIAN... \
  --asset CASSET... \
  --expected_current 800000000 \
  --amount 1000000000 \
  --report_nonce 42
```

## Safety

- Mainnet write commands require `--allow-mainnet-write`.
- Zero governance timelocks require `--allow-zero-timelock`.
- `--dry-run` prints the `stellar` commands with source-account environment overrides redacted, returns planned contract ids in the response, and never writes the manifest.
- `--json` emits stable machine-readable envelopes with `type`, `ok`, `network`, `manifest`, `commands`, `tx_hashes`, `warnings`, and command-specific `data`.
- `--json-lines` emits the same envelope format as newline-delimited JSON for long-running automation.
- Contract writes run a Stellar preflight simulation before submission: invokes use `--send no`, while deploy/upload transactions use `--build-only` followed by `stellar tx simulate`. The CLI prints simulation output to stderr, including Stellar-reported auth, footprint/resource, fee, and contract-error details when the Stellar CLI provides them; JSON modes keep stdout machine-readable and surface preflight failures as structured command errors.
- Prefer Stellar keystore identities: run `stellar keys use <identity>` in the mounted/configured Stellar config directory, or pass a non-secret identity alias/public account with `--source-account`.
- Do not pass raw secret keys or seed phrases to `--source-account`; the CLI rejects obvious secret material there. If an operator must use an ephemeral secret, set `STELLAR_ACCOUNT` for the `stellar` child process environment instead of putting it in CLI argv.
- Source-account overrides use `secrecy`/`zeroize` wrappers, are redacted from command displays and tracing logs, are zeroized from in-process environment override copies after use, and are never persisted to the deployment manifest.
- Tracing logs are opt-in with `TEMPLAR_SOROBAN_VAULT_LOG` or `RUST_LOG` and are written to stderr. For example, `TEMPLAR_SOROBAN_VAULT_LOG=templar_soroban_vault_cli=info` logs deployment checkpoints and redacted Stellar command execution.
- Decimal amount flags such as `--assets 1.25 --asset-decimals 7` and `--shares 10 --share-decimals manifest` are converted to raw contract units without floating point. Exact machine callers can use raw flags such as `--assets-raw`, `--shares-raw`, and `--amount-raw`.
- Machine output is described by `tools/soroban-vault-cli/schema/output.schema.json`. Structured errors include codes such as `missing_manifest_contract`, `mainnet_guard`, and `secret_in_argv`.
- Successful non-dry-run write commands append an audit record to the manifest `transactions` list with timestamp, command/action, target contract/function when known, tx hash when visible in Stellar output, source public address when known, status, and artifact hash when applicable.
- Dangerous governance submissions such as admin rotation, timelock updates, supply queue replacement, and fee updates print a semantic old/new diff in human mode and require `--yes` or interactive confirmation before submitting. JSON/JSON-lines mode skips prompts for automation.

## Profiles

Profiles are public TOML config files for repeated local and Docker operations. They can store
network, RPC URL, passphrase, manifest path, workspace path, Stellar config directory, and default
public admin/caller/operator addresses. Do not store secret keys or seed phrases in profile files.

```sh
tmplr-soroban-vault profile init testnet
tmplr-soroban-vault --profile testnet status
```

By default, profiles live under the user config directory in
`templar/soroban-vault-cli/profiles/<name>.toml`. Set
`TEMPLAR_SOROBAN_VAULT_PROFILE_DIR` to use a project-local or Docker-mounted profile directory.
Explicit CLI flags and environment variables override profile values.

## Operator Assistance

```sh
tmplr-soroban-vault completions zsh > _tmplr-soroban-vault
tmplr-soroban-vault completions bash
tmplr-soroban-vault completions fish
tmplr-soroban-vault man > tmplr-soroban-vault.1
```

## Docker

Build the operator image from the repository root:

```sh
docker build \
  -f tools/soroban-vault-cli/Dockerfile \
  -t templar/soroban-vault-cli:local \
  .
```

The image includes `tmplr-soroban-vault`, `stellar-cli` v26, and Rust toolchains/targets for
`stellar contract build`. It defaults to
`/workspace` as the Templar workspace and persists Stellar config, deployment state, Cargo cache,
and build outputs through mount points.

```sh
docker run --rm templar/soroban-vault-cli:local --help

docker run --rm -it \
  -v "$PWD:/workspace" \
  -v "$HOME/.config/stellar:/home/templar/.config/stellar" \
  -v "$PWD/contract/vault/soroban/.deploy-state:/workspace/contract/vault/soroban/.deploy-state" \
  -v "$PWD/target:/workspace/target" \
  templar/soroban-vault-cli:local status
```

The same mount pattern supports deployment commands. Mounting the workspace and `target` directory
lets `deploy ... --build` reuse local source and build artifacts, while mounting the Stellar config
preserves identities and network configuration across runs. Use `stellar keys use <identity>` in
that config, or pass `-e STELLAR_ACCOUNT` to Docker when an ephemeral source account must come from
the environment.

## Common Operations

```sh
# User deposit through ERC-4626 proxy when configured, using decimal asset units.
tmplr-soroban-vault user deposit \
  --operator G... \
  --assets 1.25 \
  --asset-decimals 7 \
  --min-shares-out-raw 0

# Exact raw units remain available for automation.
tmplr-soroban-vault user deposit --operator G... --assets-raw 12500000

# Allocator supply through the vault compact command ABI.
tmplr-soroban-vault curator allocate-supply \
  --caller G... \
  --market 0 \
  --amount 1.25 \
  --asset-decimals 7

# Submit and optionally accept governance-backed supply queue changes.
tmplr-soroban-vault curator set-supply-queue \
  --admin G... \
  --entry 0:C...

# Submit the same typed supply queue directly to governance.
tmplr-soroban-vault governance submit-set-supply-queue \
  --admin G... \
  --entry 0:C... \
  --entry 1:C...

# Plan common governance transactions without submitting them.
tmplr-soroban-vault governance plan-submit-set-supply-queue \
  --admin G... \
  --entry 0:C...
tmplr-soroban-vault governance plan-submit-set-timelock \
  --admin G... \
  --kind supply-queue \
  --timelock-ns 86400000000000
tmplr-soroban-vault governance plan-accept --admin G... --proposal-id 7

# Inspect and progress pending governance proposals.
tmplr-soroban-vault governance queue
tmplr-soroban-vault governance explain --proposal-id 7
tmplr-soroban-vault governance accept-ready --admin G...
tmplr-soroban-vault governance submit-and-wait \
  --max-wait-seconds 3600 \
  set-timelock \
  --admin G... \
  --kind supply-queue \
  --timelock-ns 86400000000000
tmplr-soroban-vault governance submit-and-wait proposal \
  --admin G... \
  --proposal-id 7

# Update a specific governance timelock using the contract TimelockKind variants.
tmplr-soroban-vault governance submit-set-timelock \
  --admin G... \
  --kind supply-queue \
  --timelock-ns 86400000000000

# Update restrictions with a typed mode: none, blacklist, or whitelist.
tmplr-soroban-vault governance submit-set-restrictions \
  --admin G... \
  --mode whitelist \
  --accounts G...,G...

# Submit typed cap, fee, cooldown, skim, allocator, and admin handoff proposals.
tmplr-soroban-vault governance submit-set-cap --admin G... --market-id 0 --cap 1000000
tmplr-soroban-vault governance submit-set-fees \
  --admin G... \
  --performance-fee-wad 0 \
  --performance-recipient G... \
  --management-fee-wad 0 \
  --management-recipient G...

# Select the second Blend adapter by deploy order.
tmplr-soroban-vault adapter --adapter-index 1 pool

# Or select an adapter by manifest key or pool address.
tmplr-soroban-vault adapter --adapter-key blend_adapter_1 admin
tmplr-soroban-vault adapter --adapter-pool CPOOL... total-assets --asset C...

# Extend TTL for every TTL-capable contract in the manifest.
tmplr-soroban-vault extend-ttl --caller G...

# Print shell environment values from the manifest.
tmplr-soroban-vault export-env
```

`export-env` emits `BLEND_ADAPTER_ID` for the first adapter for compatibility, plus indexed
`BLEND_ADAPTER_0_ID`, `BLEND_ADAPTER_1_ID`, and matching `BLEND_POOL_0_ID` values when pool
constructor args are known. Custodial adapters use the same pattern with `CUSTODIAL_ADAPTER_ID`,
`CUSTODIAL_ADAPTER_0_ID`, matching `CUSTODIAL_ADDRESS` / `CUSTODIAL_0_ADDRESS`, and
`CUSTODIAL_0_ASSET` values when constructor args are known.

`extend-ttl` runs the vault compact `ExtendTtl` command, governance `extend_ttl`, ERC-4626 proxy
`extend_ttl`, curator proxy `extend_ttl`, share-token `extend_ttl --caller`, and each Blend adapter
or custodial adapter `extend_ttl --caller`. Manifest contracts without an explicit deployment-wide
TTL entrypoint, such as the asset token, are reported as skipped.
