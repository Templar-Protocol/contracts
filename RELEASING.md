# Releasing

Every crate in this workspace versions itself independently, from its own commit
history. Releases are automated by [release-plz](https://release-plz.dev);
configuration lives in [`release-plz.toml`](release-plz.toml).

## How to cut a release

1. Land work on `dev` as normal. Each merge updates a standing pull request
   titled **"chore: release"** (fixed via `pr_name`, so it is always findable by
   that name), which accumulates the pending version bumps,
   `Cargo.toml` edits, `Cargo.lock` updates, and `CHANGELOG.md` entries for every
   affected crate.
2. When you want to ship, review that PR and **merge it**. That is the release
   trigger — nothing is released by an ordinary merge to `dev`.
3. On merge, CI tags each released crate (`templar-gateway-client-v0.2.0`) and
   cuts its GitHub Release.

If you never merge the Release PR, nothing is ever released.

### Choosing version numbers

> **Automatic bumps are not active in the current configuration.** release-plz
> reads a crate's current version from the registry, and nothing is published,
> so every crate looks unreleased and keeps the version its `Cargo.toml` already
> states — commit messages do not move it. Set the version yourself on the
> Release PR branch with `release-plz set-version`; it updates the crate, its
> dependents and the lockfile together, and release-plz tags, changelogs and
> releases whatever you chose.
>
> This is a configuration gap, not a dead end. Running the CLI with
> `--registry-manifest-path` pointed at a checkout of the last release commit
> supplies the baseline the registry cannot, and bumps then work normally. It
> needs the CLI rather than the GitHub Action, which exposes no input for that
> flag.

Once baselines work, bumps are proposed from the commit messages since each
crate's last tag: `feat` → minor, `!` in the title → major, anything else →
patch. Any crate with a commit since its tag gets *some* bump — `chore` and
`docs` included — so a release batch is usually wider than the work you were
thinking of. See [Commit And PR Titles](AGENTS.md#commit-and-pr-titles).

Pre-1.0 crates follow Cargo's semver rules, where the minor position carries
breakage: `!` goes `0.1.0` → `0.2.0`, and a plain `feat` goes `0.1.0` → `0.1.1`
rather than to `0.2.0`. Set `features_always_increment_minor` in
`release-plz.toml` if you would rather every `feat` take the minor.

To override a proposed version, run `release-plz set-version` on the Release PR
branch and push the result:

```bash
release-plz set-version templar-gateway-core@2.0.0
```

Do not hand-edit the crate's `Cargo.toml`. Sixteen workspace dependencies pin
their path crates by version (`templar-gateway-core = { path = "./gateway/core",
version = "0.1.0" }`), so bumping only the crate leaves the root requirement
unsatisfiable and `cargo` refuses to resolve the workspace at all — the Release
PR then cannot pass the test gate it has to pass. `set-version` updates the
dependents and the lockfile with it.

There is no comment-triggered equivalent: nothing in this repo listens to
`issue_comment`, so a command posted on the PR does nothing.

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
`[workspace]` default. Registry uploads are switched off there for everything —
see the crates.io section below — so A and B differ today only in intent.

| Tier | Crates | Tag | CHANGELOG | GitHub Release | Registry |
|---|---|---|---|---|---|
| **A — published** | the 17-crate closure external consumers import | ✅ | ✅ | ✅ | ⏸ *deferred* |
| **B — tagged only** | contracts (NEAR and Soroban), `service/*`, `tools/*`, `client/vault` | ✅ | ✅ | ✅ | ❌ |
| **C — internal** | `mock/*`, `fuzz`, `test-utils`, `gateway/testing`, `contract/artifacts`, soroban integration-tests | ❌ | ❌ | ❌ | ❌ |

Tier B crates are real deliverables that ship somewhere other than a Rust build —
a deployed service, an on-chain WASM blob, a CLI image. They get a citable
version even though nobody `cargo add`s them. Tier C is build and test
scaffolding, where a version number would be noise.

Only Tier C carries `publish = false` in its own `Cargo.toml`. Tiers A and B
must not: release-plz skips a crate whose manifest forbids publishing, so
marking contracts and services that way left them with no versions, no
changelogs and no tags — and with no `*-contract-v*` tag, no canonical WASM
either. Uploads are prevented centrally by `[workspace] publish = false`
instead.

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

**The blocker is wider than publishing: it also costs automatic version bumps.**
`git_only` would derive baselines from tags rather than a registry, which is
what this repo needs. It resolves each baseline by checking the tag out and
running `cargo package`, and packaging rewrites a git dependency into a registry
one — so it demands a `redstone` on crates.io that does not exist. Giving the
dependency a `version` alongside its `git` does not help; packaging still
resolves that version against crates.io, where `redstone` is an unrelated
`0.1.0`. So `git_only` stays off, baselines come from the empty registry, and
every crate reads as unreleased at its manifest version.

Resolving RedStone would buy crates.io publishing and tag-based baselines
together. Automatic bumps do **not** have to wait for it: see the note under
[Choosing version numbers](#choosing-version-numbers) — supplying the baseline
from a local checkout avoids packaging entirely.

**To unblock**, once RedStone publishes the SDK (or we depend on a published
fork): set `redstone`'s workspace dependency to a registry version, add a
`[[package]]` block per Tier A crate with `publish = true` to override the
workspace default, and enable `semver_check`. No code changes are required.

**Check what the first run would actually upload before enabling it.** Every
Tier A crate already carries a baseline tag at its current version, so verify
with `release-plz update` on a scratch clone that the versions it proposes are
the ones you mean to publish — bumping past the baselines first is the
predictable route.

**Bump every Tier A crate past its tag, and let the first Release PR carry them
up.** Do not hand-publish the current versions to close the gap: those tags
point at trees that still carry the RedStone *git* dependency, so they cannot be
published at all, and publishing the working tree instead would bind a crates.io
version permanently to contents its identically-named tag does not contain.
`--dry-run` validates a manifest but uploads nothing, so it cannot seed the
registry either.

## Contract WASM artifacts

Releasing a **NEAR contract that `contract/artifacts` catalogues** also produces
its canonical WASM — automatically, with no manual step to forget. Soroban
contracts are tagged and released like any Tier B crate but get no WASM asset:
the catalog is NEAR-only, and `release-artifacts.yml` exits cleanly on a tag it
does not recognise.

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
up front. A `Cargo.toml` bump cannot assert one, because bumps and releases
routinely diverge — market's crate version reached 1.4.0 while 1.3.0 was the
newest release, and registry reached 1.2.1 against a released 1.1.0. Only CI,
after the fact, can write the entry: a reproducible build's digest cannot be
known until the build exists.

A catalogued entry is the **canonical build for a released version** — the bytes
CI built at that tag and published. It is not a claim that they run on any
chain; nothing here consults one. The 17 backfilled entries are the exception,
recovered from the accounts that were running them.

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
