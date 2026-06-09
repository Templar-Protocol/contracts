# Templar Soroban Vault CLI

`tmplr-soroban-vault` deploys and operates Templar Soroban vault stacks.

The CLI intentionally delegates transaction construction, simulation, signing, and submission to
the installed `stellar` CLI. It owns the Templar-specific pieces around artifact hashing,
WASM upload/reuse, deployment manifests, compact vault command payloads, and operator command
routing.

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
During non-dry-run deployment writes, the manifest is checkpointed after each successful artifact
upload/reuse decision, contract deploy/import record, asset-token record, and initialization. If a
later initialize call fails, rerunning the command can reuse the IDs already written to the manifest.

Pass `--blend-pool` once per Blend pool to deploy one adapter per pool. The manifest stores these
as `blend_adapter_0`, `blend_adapter_1`, and so on. On an existing deployment, new pools are
appended and pools already present in the manifest are left unchanged unless `--force-new` is set.

```sh
tmplr-soroban-vault deploy stack \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --blend-pool CPOOL2...
```

To add adapters later without redeploying the stack, use `deploy adapters`. If the manifest already
contains `vault` and `governance`, only the Blend adapter WASM and new adapter instances are touched.
For imported deployments, pass the existing contract ids once and the CLI records them in the
manifest before appending adapters.

```sh
tmplr-soroban-vault deploy adapters \
  --vault CVAULT... \
  --governance CGOV... \
  --asset-token CASSET... \
  --blend-pool CPOOL...
```

Use `deploy plan` to inspect reuse, upload, deploy, and manifest decisions without network writes or
manifest changes:

```sh
tmplr-soroban-vault deploy plan stack \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL...

tmplr-soroban-vault deploy plan adapters \
  --vault CVAULT... \
  --governance CGOV... \
  --blend-pool CPOOL...
```

The CLI validates Soroban account and contract addresses at parse time for operational commands.
WASM hashes accepted by governance upgrade commands must be 32-byte hex values.

## Safety

- Mainnet write commands require `--allow-mainnet-write`.
- Zero governance timelocks require `--allow-zero-timelock`.
- `--dry-run` prints the `stellar` commands with source-account environment overrides redacted, returns planned contract ids in the response, and never writes the manifest.
- `--json` emits stable machine-readable envelopes with `type`, `ok`, `network`, `manifest`, `commands`, `tx_hashes`, `warnings`, and command-specific `data`.
- `--json-lines` emits the same envelope format as newline-delimited JSON for long-running automation.
- Prefer Stellar keystore identities: run `stellar keys use <identity>` in the mounted/configured Stellar config directory, or pass a non-secret identity alias/public account with `--source-account`.
- Do not pass raw secret keys or seed phrases to `--source-account`; the CLI rejects obvious secret material there. If an operator must use an ephemeral secret, set `STELLAR_ACCOUNT` for the `stellar` child process environment instead of putting it in CLI argv.
- Source-account overrides are redacted from command displays, zeroized from in-process environment override copies after use, and never persisted to the deployment manifest.
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

The image includes `tmplr-soroban-vault`, `stellar-cli` v26, Python for the runtime
contractspec-strip step, and Rust toolchains/targets for `stellar contract build`. It defaults to
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
constructor args are known.

`extend-ttl` runs the vault compact `ExtendTtl` command, governance `extend_ttl`, ERC-4626 proxy
`extend_ttl`, curator proxy `extend_ttl`, share-token `extend_ttl --caller`, and each Blend adapter
`extend_ttl --caller`. Manifest contracts without an explicit deployment-wide TTL entrypoint, such
as the asset token, are reported as skipped.
