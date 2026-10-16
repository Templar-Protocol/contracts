# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.0](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-blend-adapter-v1.0.2...templar-soroban-blend-adapter-v1.1.0) - 2026-10-14

### Added

- *(soroban)* claim BLND emissions from blend adapter position (FIND-027)

### Documentation

- *(soroban)* note why adapter error code 8 is retired

### Fixed

- *(soroban)* drop unreachable reserve staleness check in blend adapter

### Added

- Admin-gated `claim_emissions(caller, reserve_token_ids, to)` claims BLND emissions accrued to
  the adapter's Blend position and sends them to `to` (Halborn FIND-027).

### Removed

- The 300 s reserve staleness check in `total_assets` and its `StaleReserve` error. Blend's
  `get_reserve` accrues the reserve to the current ledger, so the check could never fire.

## [1.0.2](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-blend-adapter-v1.0.1...templar-soroban-blend-adapter-v1.0.2) - 2026-10-01

### Fixed

- *(soroban)* report zero for empty Blend positions

### Fixed

- Report zero assets when the adapter has no Blend supply position, keeping vault refreshes live
  before first allocation and after a complete exit.

## [1.0.1](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-blend-adapter-v1.0.0...templar-soroban-blend-adapter-v1.0.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Fixed

- *(soroban)* allow account admins for Blend adapters
