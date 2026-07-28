# Releasing

Every crate in this workspace versions itself independently, from its own commit
history. Releases are automated by [release-plz](https://release-plz.dev);
configuration lives in [`release-plz.toml`](release-plz.toml).

## How to cut a release

1. Land work on `dev` as normal. Each merge updates a standing pull request
   titled **"chore: release"**, which accumulates the pending version bumps,
   `Cargo.toml` edits, `Cargo.lock` updates, and `CHANGELOG.md` entries for every
   affected crate.
2. When you want to ship, review that PR and **merge it**. That is the release
   trigger — nothing is released by an ordinary merge to `dev`.
3. On merge, CI tags each released crate (`templar-gateway-client-v0.2.0`) and
   cuts its GitHub Release.

If you never merge the Release PR, nothing is ever released.

### Choosing version numbers

Bumps are proposed automatically from the commit messages since each crate's
last tag (`fix` → patch, `feat` → minor, `!`/`BREAKING CHANGE` → major; see
[Commit And PR Titles](AGENTS.md#commit-and-pr-titles)). Pre-1.0 crates follow
Cargo's semver rules, so a breaking change goes `0.1.0` → `0.2.0`.

To override a proposed version, either comment on the Release PR:

```
release-plz set-version templar-gateway-core@2.0.0
```

…or edit the version directly in the PR branch.

### Editing the Release PR

The Release PR is an ordinary PR: change versions, rewrite changelog prose, or
drop a crate from the batch. It runs the full `test.yml` gate.

One behaviour to know: while the branch contains **only** bot commits, release-plz
force-pushes to keep it current. As soon as you push a **human** commit it stops
force-pushing — if more work lands on `dev`, it closes that PR and opens a fresh
one rather than overwrite your edits. So make hand-edits shortly before merging,
not days ahead.

## Release tiers

Tier C is set per-package in `release-plz.toml`; Tiers A and B share the
`[workspace]` default.

| Tier | Crates | Tag | CHANGELOG | GitHub Release | Registry |
|---|---|---|---|---|---|
| **A — published** | the 17-crate closure external consumers import | ✅ | ✅ | ✅ | ⏸ *deferred* |
| **B — tagged only** | contracts (NEAR and Soroban), `service/*`, `tools/*`, `client/vault` | ✅ | ✅ | ✅ | ❌ |
| **C — internal** | `mock/*`, `fuzz`, `test-utils`, `gateway/testing`, `contract/artifacts`, soroban integration-tests | ❌ | ❌ | ❌ | ❌ |

Tier B crates are real deliverables that ship somewhere other than a Rust build —
a deployed service, an on-chain WASM blob, a CLI image. They get a citable
version even though nobody `cargo add`s them. Tier C is build and test
scaffolding, where a version number would be noise.

## ⚠️ Publishing to crates.io is currently blocked

Tier A is configured but **not publishing yet**. `templar-common` depends on the
RedStone Rust SDK as a git dependency (`redstone`, tag `3.1.0-pre1`), which is not
on crates.io. Cargo refuses to publish any crate that has a git dependency — on
crates.io *or* a private registry — and every Tier A crate reaches
`templar-common`, so the whole closure is blocked.

Feature-gating does not help: `gateway/core/src/client/redstone_oracle.rs` and
`gateway/methods-dispatch/src/oracle_impl.rs` use
`templar_common::oracle::redstone` types directly.

**Until it is resolved, consumers pin per-crate git tags** — a real improvement
over pinning a raw commit:

```toml
templar-gateway-client = { git = "https://github.com/Templar-Protocol/contracts", tag = "templar-gateway-client-v0.1.0" }
```

**To unblock**, once RedStone publishes the SDK (or we depend on a published
fork): set `redstone`'s workspace dependency to a registry version, then add a
`[[package]]` block per Tier A crate with `git_only = false` and
`publish = true`, and enable `semver_check` at the workspace level. No code changes are required.
Verify with `cargo publish --dry-run -p <crate>` bottom-up through the closure.

## Contract WASM artifacts

Releasing a contract also produces its canonical WASM — **automatically**. There
is no manual step to forget.

1. Merge your work as normal, then merge the Release PR. That tags the version.

2. The tag fires
   [`release-artifacts.yml`](.github/workflows/release-artifacts.yml), which
   builds the contract in the pinned NEP-330 Docker image **at that tag's
   commit**, uploads `<target>-<version>.wasm` plus `checksums.txt` to the
   GitHub Release, and opens a PR adding one file under
   `contract/artifacts/releases/` — the version, the tag, the asset, and the
   digest of the bytes it just built, each recorded as observed.

3. Merge that PR. Until you do, the artifacts crate will not serve the version —
   an unrecorded release has no reviewed hash to check downloaded bytes against.

Note what is *not* in that list: nothing asks a developer to declare a release
up front. A `Cargo.toml` bump cannot assert one, because bumps and deployments
routinely diverge — market's crate version reached 1.4.0 while 1.3.0 was the
newest deployment, and registry reached 1.2.1 against a deployed 1.1.0. The
catalog records what actually shipped, so only CI, after the fact, can write it.

Bytes are Release assets, not repository content. Tests download them into a
shared cache (`just artifacts-fetch`); the pinned SHA-256 in `releases/` is
what makes a downloaded asset trustworthy. See
[`contract/artifacts/README.md`](contract/artifacts/README.md).

### Why the build happens in CI, at the tag

`cargo near build reproducible-wasm` embeds its source commit into the WASM
(NEP-330). Verifiers such as nearblocks.io read that commit back out of a
deployed contract, clone the repo at it, and rebuild — so the commit has to stay
permanently reachable. A squash-merged feature branch never becomes an ancestor
of `dev` and is eventually garbage-collected; a **tag** is reachable from a
fresh clone forever.

Building at the tag therefore makes verification trivial for anyone:

```bash
git clone https://github.com/Templar-Protocol/contracts && cd contracts
git checkout templar-proxy-oracle-near-contract-v0.3.0
cargo near build reproducible-wasm --manifest-path contract/proxy-oracle/near/contract/Cargo.toml
```

The same property is why the catalog entry lands a commit later: a reproducible
build's hash cannot be written into the commit that produces it without changing
that commit, and therefore the hash.

Releases predating this workflow were recovered from the chain — the bytes read
back off the accounts running them, each tagged at the commit its WASM names in
NEP-330 metadata — so they reproduce on the same terms. Each one's GitHub
Release names the account its bytes came from.

## Setup

Both one-time steps are already done; they are recorded here because they are
what makes the rest work, not because anyone needs to repeat them.

- **Baseline tags.** Every crate was tagged at its then-current `Cargo.toml`
  version (`templar-common-v1.4.0`, …) so release-plz has a starting point.
  Without them it treats every crate as brand new and replays the entire history
  into one changelog.
- **Historical releases.** The 17 versions genuinely deployed to mainnet were
  published as GitHub Releases, tagged at their build commits. Nothing can fetch
  a released artifact that has no Release to fetch it from.

Two secrets must be present:

- **`RELEASE_PLZ_TOKEN`** (required). A PR opened with the default
  `GITHUB_TOKEN` does not trigger other workflows, so without a PAT the Release
  PR never runs `test.yml` and the release tags never run the artifact/CLI
  workflows. Both release-plz jobs fail fast if it is unset.
- **`CARGO_REGISTRY_TOKEN`.** Only consulted once Tier A leaves `git_only` mode.
