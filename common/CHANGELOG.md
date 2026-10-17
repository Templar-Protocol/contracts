# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.3.0](https://github.com/Templar-Protocol/contracts/compare/templar-common-v2.2.0...templar-common-v2.3.0) - 2026-10-17

### Added

- *(manager)* add typed live fleet queries ([#681](https://github.com/Templar-Protocol/contracts/pull/681))

### Fixed

- *(soroban)* reject deposits against stale external NAV (FIND-001) ([#665](https://github.com/Templar-Protocol/contracts/pull/665))

### Added

- Expose the market's existing utilization calculation for consistent off-chain reporting.

## [2.2.0](https://github.com/Templar-Protocol/contracts/compare/templar-common-v2.1.0...templar-common-v2.2.0) - 2026-10-14

### Added

- *(registry)* emit events for versions and deployments ([#658](https://github.com/Templar-Protocol/contracts/pull/658))

### Changed

- *(oracle)* share Lazer feed projection (ENG-776) ([#654](https://github.com/Templar-Protocol/contracts/pull/654))
- *(registry)* drop pre-2.0.0 migrations and market wording ([#656](https://github.com/Templar-Protocol/contracts/pull/656))

### Fixed

- *(market)* bound withdrawal queue batch processing ([#659](https://github.com/Templar-Protocol/contracts/pull/659))

## [2.1.0](https://github.com/Templar-Protocol/contracts/compare/templar-common-v2.0.0...templar-common-v2.1.0) - 2026-10-01

### Added

- *(market)* expose supplier yield cash pool (ENG-665) ([#625](https://github.com/Templar-Protocol/contracts/pull/625))

### Fixed

- *(common)* align market ABI schemas (ENG-696) ([#629](https://github.com/Templar-Protocol/contracts/pull/629))

## [2.0.0](https://github.com/Templar-Protocol/contracts/compare/templar-common-v1.5.0...templar-common-v2.0.0) - 2026-08-24

### Added

- *(common)* registry view types for sound target and version checks (ENG-559, ENG-560) ([#587](https://github.com/Templar-Protocol/contracts/pull/587))
- *(gateway)* [**breaking**] factor out the shared deploy-from-registry op structure (ENG-466) ([#594](https://github.com/Templar-Protocol/contracts/pull/594))
- *(registry)* [**breaking**] register an existing global contract by code hash (ENG-631) ([#596](https://github.com/Templar-Protocol/contracts/pull/596))

### Fixed

- *(proxy-oracle)* remediate Halborn findings ([#595](https://github.com/Templar-Protocol/contracts/pull/595))

## [1.5.0](https://github.com/Templar-Protocol/contracts/compare/templar-common-v1.4.1...templar-common-v1.5.0) - 2026-08-04

### Added

- declarative market deployment — one spec, plan/apply, real preflight (ENG-537) ([#540](https://github.com/Templar-Protocol/contracts/pull/540))

## [1.4.1](https://github.com/Templar-Protocol/contracts/compare/templar-common-v1.4.0...templar-common-v1.4.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Documentation

- fix rustdoc warnings and gate them in CI ([#532](https://github.com/Templar-Protocol/contracts/pull/532))
