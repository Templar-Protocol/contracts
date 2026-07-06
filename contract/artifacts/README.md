# templar-contract-artifacts

Canonical contract artifact IDs, metadata, and byte-loading helpers for
Templar Protocol smart contracts.

## What this crate provides

- **Artifact catalog** — a single source of truth for every deployable
  contract in the workspace (production contracts and mock/test contracts).
- **Metadata** — Cargo package name, `target/near` directory name, and
  workspace-relative source path for each artifact.
- **Version-key helpers** — format and hash helpers matching the
  `{package}@{version}#{sha256_hex}` convention from `templar-tools-common`.
- **Byte-loading** — two mutually independent features for obtaining
  compiled WASM bytes.

## Features

| Feature              | What it enables                                                   |
|----------------------|-------------------------------------------------------------------|
| *(default)*          | Artifact IDs and metadata only. No dependencies beyond `sha2`, `hex`, `thiserror`. No WASM bytes. |
| `workspace-loader`   | Read WASM from `target/near/{name}/{name}.wasm` at runtime. Provides `cargo near build` helper. |
| `embedded-wasm`      | Compile-time WASM blobs via `include_bytes!`. Blobs are pinned at build time. |
| `clap`               | CLI-friendly parsing helpers (parse artifacts by friendly name or package name). |

Default features do **not** embed WASM bytes or depend on heavy build
tooling. Consumers opt into the byte source they need.

## Why no `build.rs`

This crate deliberately does **not** compile contracts in a build script.
Contract compilation is performed by `./script/prebuild-test-contracts.sh`
or by `cargo near build`. The crate only *reads* the resulting artifacts.

## Embedded WASM and staleness

When the `embedded-wasm` feature is active, every `include_bytes!` call
reads from **checked-in files** under `contract/artifacts/res/near/`.
These blobs are pinned in version control and only updated when the
prebuild script is re-run and the fresh output is copied into `res/near/`.

All 14 catalogued artifacts (production and mock) have embedded bytes
available.

### Checking for stale bytes

Run the drift check (requires prebuilt artifacts in `target/near`):

```bash
cargo test -p templar-contract-artifacts \
  --features embedded-wasm,workspace-loader \
  drift_check -- --ignored --nocapture
```

This runs both the **byte drift check** (compares embedded blobs against
`target/near`) and the **version drift check** (verifies catalog versions
match `Cargo.toml`), because `drift_check` is a substring filter matching
`embedded_drift_check` and `embedded_version_drift_check`.

If either test fails, the checked-in bytes or catalog versions need updating.
Fix by:

1. Run `./script/prebuild-test-contracts.sh`
2. Copy fresh WASM from `target/near/` to `contract/artifacts/res/near/`
3. Rebuild the crate
4. Run the drift check again

The CI script `./script/test.sh` runs this drift check automatically after
prebuilding contracts.

## Usage examples

### Just metadata (default features)

```rust
use templar_contract_artifacts::{artifact_catalog, find_by_package_name};

let catalog = artifact_catalog();
let market = find_by_package_name("templar-market-contract").unwrap();
assert_eq!(market.source_path, "contract/market");
```

### Load WASM from workspace build directory

```rust
// Requires: features = ["workspace-loader"]
use templar_contract_artifacts::{find_by_package_name, load_artifact_bytes};

let meta = find_by_package_name("templar-market-contract").unwrap();
let bytes = load_artifact_bytes(Path::new("/path/to/workspace"), meta)?;
```

### Format a version key

```rust
use templar_contract_artifacts::format_version_key;

let key = format_version_key("mock-ft", "0.0.0", &wasm_bytes);
// => "mock-ft@0.0.0#<64-char sha256 hex>"
```

### CLI parsing with clap

```rust
// Requires: features = ["clap"]
// In your clap derive struct:
#[arg(value_parser = templar_contract_artifacts::parse_artifact_id)]
artifact: templar_contract_artifacts::ContractArtifact,
```

## Artifact list

| Friendly name       | Cargo package                          | `target/near` directory             |
|---------------------|----------------------------------------|-------------------------------------|
| registry            | templar-registry-contract              | templar_registry_contract           |
| market              | templar-market-contract                | templar_market_contract             |
| vault               | templar-vault-contract                 | templar_vault_contract              |
| universal-account   | templar-universal-account-contract      | templar_universal_account_contract  |
| proxy-oracle        | templar-proxy-oracle-near-contract     | templar_proxy_oracle_near_contract  |
| proxy-governance    | templar-proxy-oracle-near-governance-contract | templar_proxy_oracle_near_governance_contract |
| lst-oracle          | templar-lst-oracle-contract            | templar_lst_oracle_contract         |
| redstone-adapter    | templar-redstone-adapter-contract      | templar_redstone_adapter_contract   |
| pyth-pro-adapter    | templar-pyth-pro-adapter-contract       | templar_pyth_pro_adapter_contract   |
| mock-ft             | mock-ft                                | mock_ft                             |
| mock-mt             | mock-mt                                | mock_mt                             |
| mock-oracle         | mock-oracle                            | mock_oracle                         |
| mock-ref-finance    | mock-ref                               | mock_ref                            |
| mock-receiver       | mock-receiver                          | mock_receiver                       |
