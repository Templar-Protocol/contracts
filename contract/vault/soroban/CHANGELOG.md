# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.0](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-runtime-v1.1.1...templar-soroban-runtime-v1.2.0) - 2026-10-14

### Added

- *(soroban)* claim BLND emissions from blend adapter position (FIND-027)

### Changed

- *(soroban-vault)* share supply cap enforcement
- *(vault)* share checked fee accrual math

### Documentation

- *(soroban)* note why adapter error code 8 is retired

### Fixed

- *(soroban-vault)* enforce allocation exposure caps (ENG-698)
- *(soroban-vault)* enforce refresh exposure caps
- *(soroban-vault)* record NAV beyond supply admission caps
- *(soroban-vault)* crystallize queued withdrawal fees (ENG-699)
- *(vault)* crystallize fees before redemptions
- *(soroban-vault)* crystallize fees before idle reconciliation (ENG-700)
- *(vault)* crystallize fees before idle reconciliation
- *(soroban-vault)* accrue same-ledger fees before reconciliation
- *(soroban)* drop unreachable reserve staleness check in blend adapter
- *(soroban)* halt idle-balance resync while paused (FIND-007)
- *(soroban)* halt RefreshFees while paused

### Performance

- *(soroban-vault)* fit rebased runtime under host limit

### Fixed

- `ResyncIdleBalance` and `RefreshFees` are rejected with `EnforcedPause` while the vault is
  paused; a rejected resync does not consume the resync cooldown (Halborn FIND-007).
- Fee recipients may no longer be the vault itself or its asset/share token contracts; such a
  change is rejected with `InvalidInput` (Halborn FIND-009).
- A queued withdrawal whose receiver cannot accept the asset (no or deauthorized trustline)
  now settles as a failed payout: escrow is refunded, nothing is burned, the head is
  dequeued, and `ExecuteWithdraw` returns `Completed` with zero `assets_out`. Previously the
  SAC transfer trapped and the queue was wedged behind that head. The token adapter uses
  `try_transfer` and reports failures as `EffectFailed` (Halborn FIND-015).

## [1.1.1](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-runtime-v1.1.0...templar-soroban-runtime-v1.1.1) - 2026-10-01

### Fixed

- *(soroban)* report zero for empty Blend positions

## [1.1.0](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-runtime-v1.0.0...templar-soroban-runtime-v1.1.0) - 2026-08-03

### Added

- soroban curator kernel ([#378](https://github.com/Templar-Protocol/contracts/pull/378))
- *(soroban)* emit typed kernel contract events
- *(proxy-4626)* add Soroban ERC-4626 proxy
- *(soroban)* split idle and async withdrawals (#FIND-002 Nexus 39906066-2ebb-4b87-9e22-844ce7913a9c)
- *(proxy-curator)* add Soroban curator operations proxy
- *(soroban)* return typed execute receipts
- custodial market adapter (soroban)
- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Changed

- vault ergonomics ([#401](https://github.com/Templar-Protocol/contracts/pull/401))
- *(soroban-cli)* simplify redeployment guards (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* split command module ([#534](https://github.com/Templar-Protocol/contracts/pull/534))

### Documentation

- *(soroban)* document ttl keeper responsibilities
- *(soroban)* clarify queued withdrawal ergonomics
- *(soroban)* state atomic exit priority model
- *(soroban)* clarify immediate atomic exit wording
- fix rustdoc warnings and gate them in CI ([#532](https://github.com/Templar-Protocol/contracts/pull/532))
- *(soroban)* align Blend admin security guidance
- *(soroban)* clarify Blend admin eligibility

### ENG-484

- add custodial NAV report timestamps ([#529](https://github.com/Templar-Protocol/contracts/pull/529))
- expose vault version capabilities and replace curator proxy ([#530](https://github.com/Templar-Protocol/contracts/pull/530))

### Fixed

- *(proxy-4626)* type proxy view response
- *(proxy-4626)* use infallible proxy view conversion
- *(soroban)* make governance ttl renewal permissionless
- *(soroban)* reject partial idle queued withdrawals (#FIND-002 Nexus 39906066-2ebb-4b87-9e22-844ce7913a9c)
- *(soroban)* keep allocation finish from auto-withdrawing
- allow vault escrow under share-token whitelist
- *(curator-proxy)* decode foreign vault errors safely
- *(curator-proxy)* flatten downstream contract errors
- *(curator-proxy)* reject nonpositive allocations
- *(soroban-cli)* [**breaking**] require explicit adapter admin
- *(soroban)* preserve explicit deployment admins (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* trim redeployment safeguards (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban)* close deployment authority gaps (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban)* allow account admins for Blend adapters
