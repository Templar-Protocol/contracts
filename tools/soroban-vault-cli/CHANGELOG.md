# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-vault-cli-v0.1.0...templar-soroban-vault-cli-v0.2.0) - 2026-08-03

### Added

- *(release)* [**breaking**] automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Changed

- *(soroban-cli)* simplify redeployment guards (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* split command module ([#534](https://github.com/Templar-Protocol/contracts/pull/534))

### Documentation

- *(soroban)* align Blend admin security guidance

### ENG-484

- expose vault version capabilities and replace curator proxy ([#530](https://github.com/Templar-Protocol/contracts/pull/530))

### Fixed

- *(soroban-cli)* [**breaking**] require explicit adapter admin
- *(soroban)* preserve explicit deployment admins (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* harden fresh redeploy safety (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* trim redeployment safeguards (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban)* close deployment authority gaps (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* reject recorded governance on force-new
- *(soroban)* allow account admins for Blend adapters
