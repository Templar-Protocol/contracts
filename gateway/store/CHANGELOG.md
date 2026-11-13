# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-store-v0.2.6...templar-gateway-store-v0.3.0) - 2026-11-13

### Fixed

- *(gateway)* [**breaking**] count unloadable operations in startup recovery instead of aborting (ENG-476) ([#720](https://github.com/Templar-Protocol/contracts/pull/720))
- *(gateway)* decode stored prepared signed transactions (ENG-478) ([#721](https://github.com/Templar-Protocol/contracts/pull/721))

### Fixed

- Decode canonical NEAR V0 and V1 signed transactions when reloading Prepared operations from Postgres (ENG-478). Existing stored bytes need no migration.

## [0.2.3](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-store-v0.2.2...templar-gateway-store-v0.2.3) - 2026-09-01

### Fixed

- *(gateway)* settle a transaction the chain never recorded (ENG-477) ([#611](https://github.com/Templar-Protocol/contracts/pull/611))
- *(gateway)* ask the chain what it has, not what it will have (ENG-674) ([#613](https://github.com/Templar-Protocol/contracts/pull/613))

## [0.2.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-store-v0.1.2...templar-gateway-store-v0.2.0) - 2026-08-04

### Added

- *(gateway-core)* [**breaking**] serialize the write path per access key (ENG-530) ([#561](https://github.com/Templar-Protocol/contracts/pull/561))

## [0.1.1](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-store-v0.1.0...templar-gateway-store-v0.1.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Documentation

- fix rustdoc warnings and gate them in CI ([#532](https://github.com/Templar-Protocol/contracts/pull/532))
