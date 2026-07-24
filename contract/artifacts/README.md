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

## Versioned release blobs

Contract bytes live under:

```
res/near/<cargo_target_name>/<version>/<cargo_target_name>.wasm
```

One directory per **released** version. Releases are **immutable**: cutting a
new one *adds* a directory and a catalog entry, and never rewrites an existing
one. Historical blobs are what the migration and upgrade tests deploy — e.g.
`contract/universal-account/tests/migration.rs` upgrades from the real `0.2.0`
and `0.4.0` binaries — so rewriting one silently invalidates those tests.

Each entry in `ArtifactId::metadata().releases` (oldest first) pins a version and
the SHA-256 of its blob. `ArtifactMetadata::current()` is the newest release —
what the gateway deploys, and what `version()` / `expected_sha256()` /
`version_key()` refer to.

```rust
// Newest released bytes.
let bytes = ArtifactId::Market.embedded_bytes();

// A specific historical release, for upgrade tests.
let old = ArtifactId::UniversalAccount.embedded_bytes_for_version("0.2.0");
```

### The prebuild helper (test artifacts)

`./script/prebuild-test-contracts.sh` builds contracts into Cargo's
`target/near/` for the **test suite** (via `TEST_CONTRACTS_PREBUILT=1`). It uses
fast, non-reproducible `cargo near build` and never touches `res/near/`.

Set `PREBUILD_TEST_CONTRACTS_JOBS=<n>` to control build concurrency. Set
`PREBUILD_TEST_CONTRACTS_TIMEOUT_SECS=<n>` or pass `--timeout-secs <n>` to
override the per-contract timeout (default 30 minutes). Pass `--artifact <name>`
to build a subset (repeatable or comma-separated). Pass `--check` to report which
artifacts are missing from `target/near` and exit non-zero without building.

```bash
./script/prebuild-test-contracts.sh --artifact market
./script/prebuild-test-contracts.sh --artifact market,mock-ft
```

## Cutting a release

Source is *allowed* to move ahead of the newest released blob — unreleased
work-in-progress is meant to lag it. The tripwire is the crate version:

**Bump a contract's `Cargo.toml` version when you intend to ship it.** The drift
check then fails (newest catalogued release ≠ `Cargo.toml` version) until you cut
the blob:

```bash
just artifact-release proxy-oracle
```

That script requires a **clean, committed tree** (`cargo near build
reproducible-wasm` builds from committed git state), builds the contract
reproducibly, installs it at `res/near/<target>/<version>/`, and prints the
catalog entry to add to `ids.rs`. Add the entry *and* a matching
`embedded_bytes_for_version` arm, then commit the blob together with the catalog
edit so bytes and pin always land in one reviewable diff.

It refuses to overwrite an existing version — to ship new bytes, bump the
version.

## Checking consistency

```bash
./script/check-artifact-drift.sh
```

Pure, in-memory, no builds, seconds. Four guarantees:

| Check | Catches |
|---|---|
| `embedded_drift_check` | a release's bytes no longer hash to its pinned `sha256` (including historical releases, which must never change) |
| `embedded_version_drift_check` | the newest release does not match the crate's `Cargo.toml` version — i.e. a version bump whose blob was never cut |
| `catalog_releases_are_well_formed` | empty release lists, duplicate versions, malformed digests |
| `catalog_matches_disk` | a directory on disk nobody catalogued, or a catalog entry whose blob was never committed |

What this does **not** check is whether the bytes match what the source actually
compiles to — that requires a reproducible rebuild, which runs on release tags
in `.github/workflows/release-artifacts.yml` and fails unless the rebuild is
byte-for-byte identical.

### Why each release records a `source_commit`

`cargo near build reproducible-wasm` embeds the source commit into the WASM
(NEP-330), so **the same source built at two different commits produces
different bytes**. Reproducibility is only meaningful *at a specific commit*.

That is why `ArtifactRelease` carries `source_commit` and the verification
workflow rebuilds there rather than at the release tag: a blob is necessarily
committed *before* the tag that releases it exists, so verifying at the tag
could never match.

Legacy releases — bytes recovered from a deployed mainnet contract — carry
`source_commit: None`. Their hash pin is still enforced, but they cannot be
rebuilt, and the workflow says so rather than pretending to verify them.

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

See `ArtifactId::ALL` in `src/ids.rs` — the catalog is the single source of
truth, and a table here would have no drift check behind it.
