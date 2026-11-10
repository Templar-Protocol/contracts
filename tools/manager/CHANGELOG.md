# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `read-batch --json` / `--json-file` executes canonical gateway reads with at
  most four calls in flight and returns a bare array in input order. Batch
  items reject unknown fields and write methods; any error fails the command
  without publishing a partial result. Reads are not an atomic or block-pinned
  snapshot.
- Read-only `market generate-patch config` authors guarded schema-3 configuration
  patches from a complete finalized snapshot, with exact released-layout admission,
  opaque-state preservation, typed validation, and review artifacts.
- Optional `expect_json` view postconditions preserve exact nullable JSON values
  through TOML specs and stamped patch plans without changing existing encodings.
- `dao propose` and `dao get-proposal` share typed Sputnik policy, submission,
  exact returned-ID readback, and structured proposal provenance.
- MPC proposal/execution binds every approved action to one strict source plan;
  stamped patch plans reuse the complete live `patch apply` guard.
- MPC relay/broadcast retain structured transaction identity and receipt evidence
  on failed or unknown submission outcomes.
- Optional `dao mpc show --plan` and `--expected-description` bind a review to
  its exact source and requested text without requiring unspent live state.
- Paired `dao mpc show --tx-hash` / `--tx-signer` verifies the original
  signature-bearing vote against the frozen payload, including historical
  recovery after nonce consumption or expiry.
- Read-only `proxy-oracle predict-manual-trip` replays exact finalized WASM and
  complete storage in an isolated sandbox to certify the expected raw-state
  transition before a manual-trip approval.
- Snapshot exports include the resolved effective code SHA-256, including
  globally linked code, without changing the schema-3 patch plan.
- Raw MPC lifetime follows the chain's live transaction-validity period; source
  checks precede block pinning and execution rechecks the inclusion bound without
  changing approved bytes.
- Fixed-stage controlled local asset migration scripts freeze immutable campaign
  intent, use DAO/governance and source-bound MPC approvals, and reconcile actual
  resolver receipts, inventories, accounting and cached prices across restarts.
  Production submissions remain disabled pending real converter semantics and
  intermediate-state safety admission.
- Governance `create-proposal` and `execute-proposal` accept optional `--call-gas`
  for the outer governance call, retaining 300 Tgas when omitted. On creation,
  put it before `oracle`/`self`; the operation's separate `--gas` still controls
  the target call frozen in the proposal.

### Changed

- **Breaking:** move every top-level `mpc` command to `dao mpc`, without aliases.
- **Breaking:** `dao mpc propose`, `relay`, and `broadcast` require one `--plan`
  containing the exact ordinary transaction or guarded, stamped schema-3 patch.

### Fixed

- Decode successful delegated global-contract restore responses through the
  pinned canonical RPC schema fix; retain original signed bytes, canonical JSON
  output and unknown-outcome reconciliation safeguards.
- Retain original transaction outcomes in long local campaign fixtures without
  changing normal harness defaults or unknown-outcome refusal.
- Recheck full frozen cohort metadata at read-only swap/resume boundaries and
  during reconciliation, including unchanged dependencies and completed runs.
- Finalize every market-affecting configuration-patch fixture transaction and
  unsupported-code redeployment, including native refunds, before capturing
  exact no-mutation checkpoints; optimistic height advancement is not finality.
- Measure exact signed transport length without allocating a second wire buffer;
  raw broadcast validates the actual signed transport without rebuilding a duplicate estimate.

## [0.10.0](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.9.0...templar-manager-v0.10.0) - 2026-11-02

### Added

- *(deployments)* add v1 Ethena USDe/sUSDe market specs (ENG-763) ([#700](https://github.com/Templar-Protocol/contracts/pull/700))
- *(manager)* [**breaking**] register canonical contract releases ([#704](https://github.com/Templar-Protocol/contracts/pull/704))

### Changed

- **Breaking:** `registry add-version --contract/--package` now requires either
  `--release <version>` or explicit `--build`. Release registration fetches
  catalog-pinned published WASM and derives its version key automatically;
  release keys cannot be overridden. Removed `--no-build` and implicit local builds.

## [0.9.0](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.8.1...templar-manager-v0.9.0) - 2026-10-26

### Added

- *(gateway)* [**breaking**] authenticate RedStone fetches via SDK 1.0 ([#697](https://github.com/Templar-Protocol/contracts/pull/697))

## [0.8.1](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.8.0...templar-manager-v0.8.1) - 2026-10-25

### Added

- *(manager)* add typed live fleet queries ([#681](https://github.com/Templar-Protocol/contracts/pull/681))

### Added

- Query live markets, proxy oracles, and governance contracts with typed snake_case filters, nested results, non-USD defaults, relationship inclusion/mapping, explicit asset aliases, and opt-in fresh indicative USD principal values.

### Fixed

- Skip confirmed deleted accounts during live queries without marking the scan incomplete; retain failures for RPC errors and existing, unrecognized contracts.
- Compare and serialize derived numeric fields on the same Decimal grid as parsed thresholds so fractional equality, inclusive filters, and emitted-value thresholds are consistent.
- Reject USD valuations when any configured source transformation fails instead of accepting a partial-source proxy fallback.

## [0.8.0](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.7.0...templar-manager-v0.8.0) - 2026-10-14

### Added

- *(manager)* registry upgrade with a sandbox-replayed, unskippable preflight (ENG-780) ([#651](https://github.com/Templar-Protocol/contracts/pull/651))
- *(manager)* [**breaking**] use verified provider prices in preflight (ENG-777) ([#660](https://github.com/Templar-Protocol/contracts/pull/660))

### Changed

- *(gateway)* [**breaking**] drop single-field read result wrappers (ENG-772) ([#648](https://github.com/Templar-Protocol/contracts/pull/648))
- *(gateway)* drop pre-2.0.0 registry support ([#657](https://github.com/Templar-Protocol/contracts/pull/657))

## [0.7.0](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.6.0...templar-manager-v0.7.0) - 2026-10-01

### Added

- *(gateway)* [**breaking**] oracle.updateLazer accepts multiple feeds (ENG-676) ([#615](https://github.com/Templar-Protocol/contracts/pull/615))
- *(manager)* let --sign-with name the in-process backend (ENG-712) ([#628](https://github.com/Templar-Protocol/contracts/pull/628))
- *(manager)* DAO-governed MPC signing via v1.signer (ENG-718) ([#633](https://github.com/Templar-Protocol/contracts/pull/633))
- *(manager)* fold harvest-static-yield into market static-yield (ENG-771) ([#647](https://github.com/Templar-Protocol/contracts/pull/647))

### Changed

- *(mock)* mock-signer derives and signs in-contract (ENG-739) ([#637](https://github.com/Templar-Protocol/contracts/pull/637))

### Fixed

- *(manager)* accept an ambient $SECRET_KEY in --print mode (ENG-692) ([#626](https://github.com/Templar-Protocol/contracts/pull/626))
- *(manager)* read the keychain once per command ([#646](https://github.com/Templar-Protocol/contracts/pull/646))

### Performance

- *(manager)* parallelize market plan checks (ENG-769) ([#644](https://github.com/Templar-Protocol/contracts/pull/644))

## [0.6.0](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.5.1...templar-manager-v0.6.0) - 2026-09-01

### Added

- *(gateway)* [**breaking**] validate registry init args against ABI (ENG-463) ([#607](https://github.com/Templar-Protocol/contracts/pull/607))
- *(manager)* add guarded storage patch plans (ENG-652) ([#609](https://github.com/Templar-Protocol/contracts/pull/609))
- *(market)* migrate v1 markets to proxy oracles ([#614](https://github.com/Templar-Protocol/contracts/pull/614))

### Fixed

- *(manager)* harden patch dry-run replay (ENG-653) ([#612](https://github.com/Templar-Protocol/contracts/pull/612))
- *(ci)* route the patch dry-run replay to the node gate ([#616](https://github.com/Templar-Protocol/contracts/pull/616))
- *(manager)* stabilize patch replay test ([#617](https://github.com/Templar-Protocol/contracts/pull/617))
- *(manager)* isolate patch replay sandbox ([#618](https://github.com/Templar-Protocol/contracts/pull/618))

## [0.5.1](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.5.0...templar-manager-v0.5.1) - 2026-08-25

### Added

- *(manager)* gate proxy-oracle upgrades on a deployed-state preflight (ENG-648) ([#601](https://github.com/Templar-Protocol/contracts/pull/601))
- *(patch-state)* add atomic state-patching contract ([#604](https://github.com/Templar-Protocol/contracts/pull/604))

## [0.5.0](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.4.0...templar-manager-v0.5.0) - 2026-08-24

### Added

- *(manager)* make target and version preflight checks sound (ENG-559, ENG-560) ([#591](https://github.com/Templar-Protocol/contracts/pull/591))
- *(gateway)* [**breaking**] factor out the shared deploy-from-registry op structure (ENG-466) ([#594](https://github.com/Templar-Protocol/contracts/pull/594))
- *(registry)* [**breaking**] register an existing global contract by code hash (ENG-631) ([#596](https://github.com/Templar-Protocol/contracts/pull/596))

### Fixed

- *(proxy-oracle)* remediate Halborn findings ([#595](https://github.com/Templar-Protocol/contracts/pull/595))

## [0.4.0](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.3.0...templar-manager-v0.4.0) - 2026-08-07

### Added

- *(gateway)* [**breaking**] oracle.updatePyth fetches its own payload (ENG-462) ([#586](https://github.com/Templar-Protocol/contracts/pull/586))

### Fixed

- *(manager)* raise ORACLE_DEPOSIT for proxy-oracle 0.4.1 ([#582](https://github.com/Templar-Protocol/contracts/pull/582))
- *(deployments)* repoint market specs at the reorganized profiles ([#584](https://github.com/Templar-Protocol/contracts/pull/584))

## [0.3.0](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.2.0...templar-manager-v0.3.0) - 2026-08-04

### Added

- declarative market deployment — one spec, plan/apply, real preflight (ENG-537) ([#540](https://github.com/Templar-Protocol/contracts/pull/540))
- *(gateway-core)* [**breaking**] serialize the write path per access key (ENG-530) ([#561](https://github.com/Templar-Protocol/contracts/pull/561))
- *(gateway)* [**breaking**] opt into borsh governance proposals (ENG-558) ([#565](https://github.com/Templar-Protocol/contracts/pull/565))
- *(artifacts)* record released artifact byte length in the catalog ([#573](https://github.com/Templar-Protocol/contracts/pull/573))
- *(manager)* [**breaking**] readable check output, and amounts that state their unit (ENG-537) ([#575](https://github.com/Templar-Protocol/contracts/pull/575))

### Fixed

- *(manager)* raise GOVERNANCE_DEPOSIT to 4.5 NEAR (ENG-574) ([#571](https://github.com/Templar-Protocol/contracts/pull/571))

## [0.2.0](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.1.1...templar-manager-v0.2.0) - 2026-08-03

### Added

- *(proxy-oracle-governance)* [**breaking**] reflexive vs. target-function-call operations (ENG-516) ([#527](https://github.com/Templar-Protocol/contracts/pull/527))

## [0.1.1](https://github.com/Templar-Protocol/contracts/compare/templar-manager-v0.1.0...templar-manager-v0.1.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))
