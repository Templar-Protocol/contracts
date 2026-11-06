# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Atomic validated full configuration via `admin_set_config` and inline or
  immutable-global upgrades, authorized by the existing administrative RBAC role.
- Compatible `Admin` role naming with legacy Rust, JSON, CLI, and Borsh identities
  preserved; `TrustedUpdater` remains independent.
- Standard versioned state and a typed V0-to-V1 migration preserving historical
  adapter data and RBAC memberships, with direct released-WASM upgrade coverage.
- Versioned `config_updated` audit events containing the actual caller and full
  policy after validated configuration replacement; event-only hex signer encoding keeps
  the maximum 255-signer policy within the receipt log limit while preserving the
  existing configuration and `write_prices` wire formats.

## [0.2.1](https://github.com/Templar-Protocol/contracts/compare/templar-redstone-adapter-contract-v0.2.0...templar-redstone-adapter-contract-v0.2.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))
