# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.2](https://github.com/Templar-Protocol/contracts/compare/templar-vault-kernel-v1.0.1...templar-vault-kernel-v1.0.2) - 2026-10-10

### Changed

- *(vault)* share checked fee accrual math
- *(vault)* consolidate dual-basis exit quotes

### Fixed

- *(vault)* crystallize fees before redemptions
- *(soroban-vault)* crystallize fees before idle reconciliation (ENG-700)
- *(vault)* crystallize fees before idle reconciliation

## [1.0.1](https://github.com/Templar-Protocol/contracts/compare/templar-vault-kernel-v1.0.0...templar-vault-kernel-v1.0.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### ENG-484

- expose vault version capabilities and replace curator proxy ([#530](https://github.com/Templar-Protocol/contracts/pull/530))
