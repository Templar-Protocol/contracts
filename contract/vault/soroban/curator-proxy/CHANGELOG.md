# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.0](https://github.com/Templar-Protocol/contracts/compare/templar-curator-proxy-soroban-v1.0.0...templar-curator-proxy-soroban-v1.1.0) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### ENG-484

- expose vault version capabilities and replace curator proxy ([#530](https://github.com/Templar-Protocol/contracts/pull/530))

### Fixed

- *(curator-proxy)* decode foreign vault errors safely
- *(curator-proxy)* flatten downstream contract errors
- *(curator-proxy)* reject nonpositive allocations
