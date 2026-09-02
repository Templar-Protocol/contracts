# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-methods-dispatch-v0.4.1...templar-gateway-methods-dispatch-v0.5.0) - 2026-09-01

### Added

- *(gateway)* [**breaking**] validate registry init args against ABI (ENG-463) ([#607](https://github.com/Templar-Protocol/contracts/pull/607))
- *(manager)* add guarded storage patch plans (ENG-652) ([#609](https://github.com/Templar-Protocol/contracts/pull/609))
- *(market)* migrate v1 markets to proxy oracles ([#614](https://github.com/Templar-Protocol/contracts/pull/614))

## [0.4.1](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-methods-dispatch-v0.4.0...templar-gateway-methods-dispatch-v0.4.1) - 2026-08-25

### Added

- *(gateway)* reject a code-hash version against a registry too old for it (ENG-631) ([#597](https://github.com/Templar-Protocol/contracts/pull/597))
- *(gateway)* add tx.batch, a generic multi-action write op (ENG-650) ([#603](https://github.com/Templar-Protocol/contracts/pull/603))

## [0.4.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-methods-dispatch-v0.3.0...templar-gateway-methods-dispatch-v0.4.0) - 2026-08-24

### Added

- *(gateway)* serve the registry's entry and version views (ENG-559, ENG-560) ([#590](https://github.com/Templar-Protocol/contracts/pull/590))
- *(gateway)* [**breaking**] factor out the shared deploy-from-registry op structure (ENG-466) ([#594](https://github.com/Templar-Protocol/contracts/pull/594))
- *(registry)* [**breaking**] register an existing global contract by code hash (ENG-631) ([#596](https://github.com/Templar-Protocol/contracts/pull/596))

## [0.3.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-methods-dispatch-v0.2.0...templar-gateway-methods-dispatch-v0.3.0) - 2026-08-04

### Added

- declarative market deployment — one spec, plan/apply, real preflight (ENG-537) ([#540](https://github.com/Templar-Protocol/contracts/pull/540))
- *(gateway)* [**breaking**] opt into borsh governance proposals (ENG-558) ([#565](https://github.com/Templar-Protocol/contracts/pull/565))

## [0.2.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-methods-dispatch-v0.1.1...templar-gateway-methods-dispatch-v0.2.0) - 2026-08-03

### Added

- *(proxy-oracle-governance)* [**breaking**] reflexive vs. target-function-call operations (ENG-516) ([#527](https://github.com/Templar-Protocol/contracts/pull/527))

## [0.1.1](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-methods-dispatch-v0.1.0...templar-gateway-methods-dispatch-v0.1.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))
