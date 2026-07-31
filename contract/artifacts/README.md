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
| `fetch`              | Download *released* WASM from its GitHub Release into a shared local cache, verified against the catalog's SHA-256 pin. |
| `clap`               | CLI-friendly `ValueEnum` parsing for artifact IDs and package-name aliases. |

Default features do **not** embed WASM bytes or depend on heavy build
tooling. Consumers opt into the byte source they need.

## What the build script does — and does not

`build.rs` compiles `releases/` into the release lists and validates every
row, so a malformed record fails the build rather than a download. It does
**not** compile contracts: that is `./script/prebuild-test-contracts.sh` or
`cargo near build`. This crate only *reads* the resulting artifacts.

## Versioned releases

Released bytes are **GitHub Release assets, not repository content**. Each
release records the tag that carries it and the asset on that tag, in
[`releases/`](releases/) — one row per release, appended by CI.

Releases are **immutable**: cutting a new one *adds* a catalog entry and never
rewrites an existing one. Historical bytes are what the migration and upgrade
tests deploy — `contract/universal-account/tests/migration.rs` upgrades from the
exact `0.2.0` binary that ran on mainnet — so rewriting one silently invalidates
those tests.

`ArtifactMetadata::releases()` returns them oldest first;
`ArtifactMetadata::current()` is the newest — what the gateway deploys, and what
`version()` refers to. It is `None` for an artifact that has never shipped:
mocks, and (today) the NEAR vault.

The tag and asset are **recorded, not derived**. They name objects that already
exist on GitHub, and this repo has used three tag schemes over its life, so
reconstructing them from a template would assume a uniformity that has never
held. Changing release-plz's `git_tag_name` governs the *next* tag and cannot
strand the ones already cut.

**A release means a release tag was cut and its WASM published, not that a
version was bumped.**
Those diverge, routinely — market's crate version reached 1.4.0 while 1.3.0 was
the newest release, and registry reached 1.2.1 against a released 1.1.0. So
source is *expected* to run ahead of the newest release, and the catalog is
appended to by CI when a release tag is cut, never by hand.

An entry is the canonical build for a released version, **not** evidence that
those bytes run anywhere: nothing consults a chain. The 17 historical entries
are the exception — they predate this workflow and were recovered from the
accounts running them, which is why their releases name the source account.

```rust
// Requires: features = ["fetch"]
use templar_contract_artifacts::{fetch, ArtifactId};

// A specific historical release, for upgrade tests.
let old = fetch::released_bytes(ArtifactId::UniversalAccount, "0.2.0").await?;
```

### The cache

Bytes are cached outside the repository so every worktree shares one copy:

```text
${TEMPLAR_ARTIFACT_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}}/templar-contract-artifacts
  └── near/<cargo_target_name>/<version>/<cargo_target_name>.wasm
```

| Command | Does |
|---|---|
| `just artifacts-fetch` | Download every pinned release into the cache |
| `just artifacts-cache-path` | Print the resolved cache directory |
| `just artifacts-clean` | Empty it, reporting what was freed |

`TEMPLAR_ARTIFACT_OFFLINE=1` forbids downloads and restricts lookups to the
cache. `TEMPLAR_ARTIFACT_CACHE` relocates it — point it inside a checkout if you
would rather each one kept its own. Like `XDG_CACHE_HOME`, it names the
*parent*: the `templar-contract-artifacts` directory is always appended.

Cleaning is never destructive: entries are immutable release assets, so the only
cost is a re-download. Because the override names the parent, everything
`artifacts-clean` can reach sits under a directory named after this crate — a
misaimed `TEMPLAR_ARTIFACT_CACHE` cannot touch what was already there.

Sharing one cache across worktrees is safe because entries are verified against
the in-repo pin on **every read**, not just on download. A branch whose catalog
disagrees with a cached entry discards it and re-downloads — or, under
`TEMPLAR_ARTIFACT_OFFLINE=1`, fails. Either way the cache can never serve bytes
the current checkout did not ask for.

### Trust

Downloaded bytes are verified against the SHA-256 pinned in the catalog and
discarded on mismatch. That pin is a reviewed, in-repo value, so artifact
integrity does **not** rest on GitHub serving the right file — the same standard
git already gives the source, whose objects are content-addressed and mirrored
by every clone.

A version that has not been recorded as a release cannot be fetched at all:
there is no reviewed hash to check it against.

### The prebuild helper (test artifacts)

`./script/prebuild-test-contracts.sh` builds contracts into Cargo's
`target/near/` for the **test suite** (via `TEST_CONTRACTS_PREBUILT=1`). It uses
fast, non-reproducible `cargo near build`; these artifacts are never released.

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

**There is nothing to do by hand.** Merging the release PR tags the version;
`.github/workflows/release-artifacts.yml` builds the WASM reproducibly at that
tag, uploads it, and opens a PR adding one file under `releases/`. Until that
PR merges, `fetch` will not serve the version — an unrecorded release has no
reviewed hash to check downloaded bytes against.

Why the build happens at the tag, and why the catalog row necessarily lands one
commit later, are explained in [RELEASING.md](../../RELEASING.md#contract-wasm-artifacts).

Releases predating this workflow were recovered from the chain and are
reproducible on the same terms. Several were built from paths that have since
moved (proxy-oracle from `contract/proxy-oracle`, the LST oracle from
`contract/lst-oracle`); a verifier reads the historical path from the WASM's own
`build_info`, not from `source_path`.

## Checking consistency

```bash
./script/check-artifact-drift.sh
```

Seconds, no contract builds:

| Check | Catches |
|---|---|
| `no_release_is_ahead_of_its_source` | a release claiming a version the crate never reached (the reverse — source ahead of the newest release — is normal) |
| `mocks_are_never_released` | a mock that acquired a release |
| `scaffolding_crates_are_excluded_from_releases` | a Tier C crate that lost its `release = false` |

Each file's own shape — column count, canonical artifact and version spelling,
URL-safe tag and asset, digest, and agreement with its filename — is checked by
`build.rs`, so a malformed record fails the build rather than a download.

What this does **not** check is whether the bytes match what the source actually
compiles to. That needs a reproducible rebuild, which runs on release tags in
`.github/workflows/release-artifacts.yml`.


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
