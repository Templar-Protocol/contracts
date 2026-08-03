# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0](https://github.com/Templar-Protocol/contracts/compare/templar-proxy-oracle-near-contract-v0.3.0...templar-proxy-oracle-near-contract-v0.4.0) - 2026-08-03

### Added

- kernelize proxy oracle ([#403](https://github.com/Templar-Protocol/contracts/pull/403))
- *(gateway)* in-process Pyth Lazer payload source + write path (ENG-398, ENG-400) ([#491](https://github.com/Templar-Protocol/contracts/pull/491))
- *(proxy-oracle)* `new` accepts an optional owner id (ENG-440) ([#504](https://github.com/Templar-Protocol/contracts/pull/504))
- *(gateway)* configure transaction finality policy (ENG-468) ([#517](https://github.com/Templar-Protocol/contracts/pull/517))
- *(proxy-oracle)* standardize contract self-upgrade (ENG-481) ([#519](https://github.com/Templar-Protocol/contracts/pull/519))
- *(release)* [**breaking**] automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Changed

- *(pyth-pro)* drop PriceIdentifier feed map; adapter serves raw FeedData (ENG-434) ([#494](https://github.com/Templar-Protocol/contracts/pull/494))
- *(pyth-lazer)* rename all "Pyth Pro" terminology to "Pyth Lazer" repo-wide (ENG-437) ([#496](https://github.com/Templar-Protocol/contracts/pull/496))

### ENG-486

- Upgrade proxy oracles from v0.1.0 to v0.3.0 ([#507](https://github.com/Templar-Protocol/contracts/pull/507))
