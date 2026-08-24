# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
