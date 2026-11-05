# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Explicit administrator ownership, atomic validated full configuration via
  `admin_set_config`, and owner-controlled inline or immutable-global upgrades.
- One-time self-only ownership bootstrap preserving historical adapter state and
  independent RBAC; released-WASM and keyless governance regression coverage.
- Versioned `config_updated` audit events containing the actual caller and full
  policy after validated configuration replacement; event-only hex signer encoding keeps
  the maximum 255-signer policy within the receipt log limit while preserving the
  existing configuration and `write_prices` wire formats.

## [0.2.1](https://github.com/Templar-Protocol/contracts/compare/templar-redstone-adapter-contract-v0.2.0...templar-redstone-adapter-contract-v0.2.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))
