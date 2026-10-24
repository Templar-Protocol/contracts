# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.3.0-rc.1](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-runtime-v1.2.0...templar-soroban-runtime-v1.3.0-rc.1) - 2026-10-24

### Added

- soroban curator kernel ([#378](https://github.com/Templar-Protocol/contracts/pull/378))
- *(soroban)* emit typed kernel contract events
- *(proxy-4626)* add Soroban ERC-4626 proxy
- *(soroban)* split idle and async withdrawals (#FIND-002 Nexus 39906066-2ebb-4b87-9e22-844ce7913a9c)
- *(proxy-curator)* add Soroban curator operations proxy
- *(soroban)* return typed execute receipts
- custodial market adapter (soroban)
- *(release)* [**breaking**] automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))
- *(soroban)* claim BLND emissions from blend adapter position (FIND-027)

### Changed

- vault ergonomics ([#401](https://github.com/Templar-Protocol/contracts/pull/401))
- *(soroban-cli)* simplify redeployment guards (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* split command module ([#534](https://github.com/Templar-Protocol/contracts/pull/534))
- *(soroban-vault)* share supply cap enforcement
- *(vault)* share checked fee accrual math

### Documentation

- *(soroban)* document ttl keeper responsibilities
- *(soroban)* clarify queued withdrawal ergonomics
- *(soroban)* state atomic exit priority model
- *(soroban)* clarify immediate atomic exit wording
- fix rustdoc warnings and gate them in CI ([#532](https://github.com/Templar-Protocol/contracts/pull/532))
- *(soroban)* align Blend admin security guidance
- *(soroban)* clarify Blend admin eligibility
- *(soroban)* note why adapter error code 8 is retired
- *(soroban)* v1.3.0-rc.1 upgrade runbook and changelog correction
- *(soroban)* record the v1.2.0 upgrade rehearsal

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
- *(soroban)* report zero for empty Blend positions
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
- *(soroban)* settle failed payout transfers instead of trapping (FIND-015)
- *(soroban)* bound Other approvals and proposal execution; honor per-kind timelock decrease (FIND-024, FIND-026, FIND-022) ([#667](https://github.com/Templar-Protocol/contracts/pull/667))
- *(soroban)* reject deposits against stale external NAV (FIND-001) ([#665](https://github.com/Templar-Protocol/contracts/pull/665))
- *(soroban)* write negative adapter valuations down to zero (FIND-013) ([#672](https://github.com/Templar-Protocol/contracts/pull/672))
- *(soroban)* settle accrued fees before applying a fee change (FIND-008) ([#666](https://github.com/Templar-Protocol/contracts/pull/666))
- *(soroban)* reject the vault and its tokens as fee recipients (FIND-009) ([#670](https://github.com/Templar-Protocol/contracts/pull/670))
- *(soroban)* reject binding one adapter to multiple markets (FIND-019) ([#669](https://github.com/Templar-Protocol/contracts/pull/669))
- *(soroban)* let a full market exit realize more than its principal (FIND-020) ([#671](https://github.com/Templar-Protocol/contracts/pull/671))
- *(soroban-cli)* preflight and cutover guards fail closed on real chain state
- *(soroban-cli)* address review on upgrade tooling
- *(soroban)* read Blend reserve fields by name in the blend adapter

### Performance

- *(soroban-vault)* fit rebased runtime under host limit

### Fixed

- Deposits are rejected (`InvalidState`) while the vault holds external assets and no
  `RefreshMarkets` covering every market with principal ran within the governance-configurable
  `MaxNavAge` window (default 24 h; kind 10), so shares are not priced against a stale NAV
  (Halborn FIND-001).
- A negative adapter valuation is written down to zero on refresh and supply observation
  instead of aborting the call with `InvalidInput` (Halborn FIND-013).
- A governance fee change now settles fees accrued under the previous spec to the previous
  recipients and re-anchors, so the new rate and recipients apply only going forward. The
  change requires an idle vault (Halborn FIND-008).
- Fee recipients may no longer be the vault itself or its asset/share token contracts; such a
  change is rejected with `InvalidInput`, and `migrate` rejects a previously stored conflicting
  recipient the same way before clearing the migration flag. A fee change replacing such a
  stored recipient forfeits the accrued window instead of minting it to that recipient
  (Halborn FIND-009).
- A queued withdrawal whose receiver cannot accept the asset (no or deauthorized trustline)
  now settles as a failed payout: escrow is refunded, nothing is burned, the head is
  dequeued, and `ExecuteWithdraw` returns `Completed` with zero `assets_out` — an expected
  outcome of a failed payout, not a zero-progress defect. Operators seeing such a receipt
  should follow the README guidance: alert and unblock the receiver's trustline before the
  user re-requests. Previously the SAC transfer trapped and the queue was wedged behind
  that head. The token adapter uses `try_transfer` and reports failures as `EffectFailed`
  (Halborn FIND-015).
- `SetSupplyQueue` rejects binding an adapter that is already bound to a different market, so one
  adapter position can no longer be counted once per market (Halborn FIND-019).
- A market withdrawal no longer underflows when it realizes more than the recorded principal.
  The withdrawal observes the adapter's remaining value, marks the market at
  `realized + remaining` as a refresh would (recognizing any gain for the performance fee),
  then keeps the observed remainder as principal (Halborn FIND-020).

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
