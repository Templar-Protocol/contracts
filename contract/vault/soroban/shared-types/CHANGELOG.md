# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.0](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-shared-types-v1.0.0...templar-soroban-shared-types-v1.1.0) - 2026-08-03

### Added

- soroban curator kernel ([#378](https://github.com/Templar-Protocol/contracts/pull/378))
- *(soroban)* split idle and async withdrawals (#FIND-002 Nexus 39906066-2ebb-4b87-9e22-844ce7913a9c)
- *(proxy-curator)* add Soroban curator operations proxy
- *(soroban)* return typed execute receipts
- *(release)* [**breaking**] automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Changed

- vault ergonomics ([#401](https://github.com/Templar-Protocol/contracts/pull/401))

### ENG-484

- expose vault version capabilities and replace curator proxy ([#530](https://github.com/Templar-Protocol/contracts/pull/530))

### Fixed

- *(proxy-4626)* type proxy view response
- *(proxy-4626)* use infallible proxy view conversion
