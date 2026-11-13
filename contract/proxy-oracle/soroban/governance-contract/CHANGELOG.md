# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Reserve Admin recovery capacity; restrict cancellation to creator/Admin and purge newly unauthorized proposals after revocation.
- Require bootstrap and newly granted Admin authorization, retaining last-Admin protection.
- Map returned runtime failures atomically and maintain all role storage TTL key families through bounded keeper pages.
- Group unchanged proposal bodies into eight bounded persistent entries so sparse full-queue revocation fits transaction limits. This storage release is fresh-stack only; existing governance deployments are not migrated.

### Added

- Cancel a pending ownership transfer under the existing Admin transfer timelock.

## [0.1.3](https://github.com/Templar-Protocol/contracts/compare/templar-proxy-oracle-soroban-governance-contract-v0.1.2...templar-proxy-oracle-soroban-governance-contract-v0.1.3) - 2026-08-24

### Fixed

- *(proxy-oracle)* remediate Halborn findings ([#595](https://github.com/Templar-Protocol/contracts/pull/595))

## [0.1.1](https://github.com/Templar-Protocol/contracts/compare/templar-proxy-oracle-soroban-governance-contract-v0.1.0...templar-proxy-oracle-soroban-governance-contract-v0.1.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))
