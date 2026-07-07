# templar-contract-artifacts

Canonical contract artifact IDs, metadata, and byte-loading helpers for
Templar Protocol smart contracts.

## What this crate provides

- **Artifact IDs** — a single source of truth for every deployable contract in
  the workspace (production contracts and mock/test contracts) via
  `ArtifactId::ALL`.
- **Metadata** — Cargo package name, `target/near` directory name, and
  workspace-relative source path for each artifact through infallible
  `ArtifactId::metadata()` lookup.
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
| `clap`               | CLI-friendly `ValueEnum` parsing for artifact IDs and package-name aliases. |

Default features do **not** embed WASM bytes or depend on heavy build
tooling. Consumers opt into the byte source they need.

## Why no `build.rs`

This crate deliberately does **not** compile contracts in a build script.
Contract compilation is performed by `./script/prebuild-test-contracts.sh`
or by `cargo near build`. The crate only *reads* the resulting artifacts.

## Embedded WASM and staleness

When the `embedded-wasm` feature is active, every `include_bytes!` call
reads from **checked-in files** under `contract/artifacts/res/near/`.
These blobs are pinned in version control and treated as versioned, immutable
release artifacts: source is free to move ahead of a shipped blob, and a blob is
only replaced when you deliberately cut new bytes (see below).

### The prebuild helper (test artifacts)

`./script/prebuild-test-contracts.sh` builds contracts into Cargo's
`target/near/` for the **test suite** (via `TEST_CONTRACTS_PREBUILT=1`). It uses
fast, non-reproducible `cargo near build` and never touches the checked-in
`res/near/` blobs.

Set `PREBUILD_TEST_CONTRACTS_JOBS=<n>` to control how many contract builds run
concurrently. If unset, it uses a bounded default based on available CPU
parallelism. Set `PREBUILD_TEST_CONTRACTS_TIMEOUT_SECS=<n>` or pass
`--timeout-secs <n>` to override the per-contract build timeout; the default is
30 minutes. Pass `--artifact <name>` to build a subset (repeatable or
comma-separated).

```bash
./script/prebuild-test-contracts.sh --artifact market
./script/prebuild-test-contracts.sh --artifact market,mock-ft
```

All catalogued artifacts (production and mock) have embedded bytes available.

### ⚠️ Refreshing a checked-in blob — READ THIS

**The embedded blobs do NOT track your source automatically.** They are pinned,
versioned *release* artifacts: the bytes the gateway will deploy on-chain. The
contract source is free to move ahead of them, and **CI will not tell you a blob
is stale**:

- The **hash-pin check** only verifies `sha256(blob) == expected_sha256`. It
  never looks at source.
- The **version-drift check** only verifies the catalog `version` matches the
  contract's `Cargo.toml` version. It never looks at the blob's bytes.

So a contract source change at the **same version** passes CI with a stale blob.
Keeping blobs fresh is a **deliberate, manual step** — this section is the only
thing standing between you and shipping outdated contract bytes.

#### WHEN to refresh

Refresh an artifact's blob **whenever you want that contract's current source to
become what the gateway deploys** — i.e. you are promoting a source change to a
shipped/deployed release. Do **not** refresh for every source edit; unreleased
work-in-progress is *meant* to lag the blob.

Enforced tripwire: **bump the contract's `Cargo.toml` `version` whenever you make
a change you intend to ship.** That bump fails the version-drift check (catalog
`version` ≠ `Cargo.toml` version) and forces you back to this catalog — which is
exactly when you should do the full refresh below. Note the check only forces the
`version` *string* to line up; it does not verify you rebuilt the blob, so when
it fires, do the **whole** procedure, not just the version edit.

#### HOW to refresh (exact steps, per affected artifact)

1. **Commit your source change first — the git tree must be clean.**
   `cargo near build reproducible-wasm` builds from the committed git state; on a
   dirty tree it either hard-errors or embeds the wrong state and produces
   non-reproducible bytes. (For a merge: commit the merge, *then* refresh.)
2. Build reproducibly (`<source_path>` and `<target>` are the entry's
   `source_path` and `cargo_target_name` in `ids.rs`):
   ```bash
   cargo near build reproducible-wasm --manifest-path <source_path>/Cargo.toml
   ```
3. Copy the output into `res/near/`:
   ```bash
   cp target/near/<target>/<target>.wasm \
      contract/artifacts/res/near/<target>/<target>.wasm
   ```
4. In `contract/artifacts/src/ids.rs`, set that entry's `expected_sha256` to the
   new hash (printed by the build, or `sha256sum` the copied file) — and its
   `version` if the crate version changed.
5. Verify: `./script/check-artifact-drift.sh` (must be green).
6. Commit the blob **and** the `ids.rs` change together, so the bytes and their
   pinned hash always land in one reviewable diff.

### Checking for stale bytes

Run the drift check — pure, in-memory, no builds:

```bash
./script/check-artifact-drift.sh
```

It runs:

```bash
cargo test -p templar-contract-artifacts --features embedded-wasm,workspace-loader drift_check -- --include-ignored --nocapture
```

which covers both the **blob hash-pin check** (every embedded blob hashes to its
catalog `expected_sha256`) and the **version drift check** (catalog versions
match `Cargo.toml`), since `drift_check` is a substring filter matching
`embedded_drift_check` and `embedded_version_drift_check`.

If either fails, update the offending catalog entry: for a blob change, refresh
the bytes and `expected_sha256` as above; for a version mismatch, update the
`version` field.

## Usage examples

### Just metadata (default features)

```rust
use templar_contract_artifacts::{artifact_catalog, find_by_package_name};

let catalog = artifact_catalog().collect::<Vec<_>>();
let market = find_by_package_name("templar-market-contract").unwrap();
assert_eq!(market.source_path, "contract/market");
```

For ID-driven code, use the canonical ID list and infallible metadata mapping:

```rust
use templar_contract_artifacts::ArtifactId;

for id in ArtifactId::ALL {
    let metadata = id.metadata();
    assert_eq!(metadata.id, id);
}
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
#[arg(value_enum, ignore_case = true)]
artifact: templar_contract_artifacts::ArtifactId,
```

## Artifact list

| Artifact ID         | Cargo package                          | `target/near` directory             |
|---------------------|----------------------------------------|-------------------------------------|
| registry            | templar-registry-contract              | templar_registry_contract           |
| market              | templar-market-contract                | templar_market_contract             |
| vault               | templar-vault-contract                 | templar_vault_contract              |
| universal-account   | templar-universal-account-contract      | templar_universal_account_contract  |
| proxy-oracle        | templar-proxy-oracle-near-contract     | templar_proxy_oracle_near_contract  |
| proxy-governance    | templar-proxy-oracle-near-governance-contract | templar_proxy_oracle_near_governance_contract |
| lst-oracle          | templar-lst-oracle-contract            | templar_lst_oracle_contract         |
| redstone-adapter    | templar-redstone-adapter-contract      | templar_redstone_adapter_contract   |
| pyth-lazer-adapter  | templar-pyth-lazer-adapter-contract     | templar_pyth_lazer_adapter_contract |
| mock-ft             | mock-ft                                | mock_ft                             |
| mock-mt             | mock-mt                                | mock_mt                             |
| mock-oracle         | mock-oracle                            | mock_oracle                         |
| mock-ref-finance    | mock-ref                               | mock_ref                            |
| mock-receiver       | mock-receiver                          | mock_receiver                       |
