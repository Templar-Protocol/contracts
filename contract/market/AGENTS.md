# Market Contract Agent Guide

This file contains market-specific security guidance for future agents.

## Scope

The deployable market contract in `contract/market` is a thin wrapper over shared logic in `templar-common`.

Read these files together before making non-trivial changes:

- `contract/market/src/lib.rs`
- `contract/market/src/impl_helper.rs`
- `contract/market/src/impl_market_external.rs`
- `contract/market/src/impl_token_receiver.rs`
- `common/src/market/impl.rs`
- `common/src/borrow.rs`
- `common/src/supply.rs`
- `common/src/event.rs`

## Current Findings

As of 2026-03-19 (commit `44ebfbe51fb8`; PR [#382](https://github.com/Templar-Protocol/contracts/pull/382)).

- Low: excessive collateral withdrawal is enforced by underflow, not by an explicit precondition.
  In `common/src/borrow.rs`, `record_collateral_asset_withdrawal_initial` subtracts the requested collateral directly from the position and market totals without an explicit bounds check.
  In the public flow, `withdraw_collateral` reaches that path before transfer finalization, so an excessive request panics instead of returning a controlled error.
  Evidence:
  - `templar_common::borrow::BorrowPositionGuard::record_collateral_asset_withdrawal_initial` (status: open as of `44ebfbe51fb8`)
  - `contract/market/tests/collateral.rs::excessive_collateral_withdrawal` (status: open as of `44ebfbe51fb8`)
  Security impact:
  - This is not a direct fund-loss issue.
  - It is still undesirable for a public entrypoint to rely on overflow panic behavior for bounds enforcement.
  Guidance:
  - Prefer explicit `require!` guards for user-controlled bounds before mutating state.

## Important Contract Invariants

- All callback/finalize entrypoints in `contract/market/src/impl_helper.rs` are `#[private]`. Preserve that.
- The contract intentionally disables `force_unregister`; see `contract/market/src/lib.rs`.
- Position storage is charged when a supply or borrow position is first created and refunded only after cleanup when the position becomes removable.
- The deployable contract adds NEP-145 storage logic, but the core borrow/supply accounting lives in `templar-common`. Review both layers for every change.
- Borrow, collateral withdrawal, liquidation, and supply-withdrawal execution are asynchronous. Review the full initial-call plus finalize path, not only the first function.
- In the borrow flow specifically, `record_borrow_initial` mutates state before the outbound transfer. On failure, the finalize path must restore liquidity and unwind in-flight principal accounting. Do not assume that every pre-transfer mutation is rolled back: fee retention on failed borrow finalization is currently intentional policy.
- Keep market-wide accounting coherent across:
  - `borrow_asset_balance`
  - `borrow_asset_deposited_active`
  - `borrow_asset_deposited_incoming`
  - `borrow_asset_borrowed`
  - `borrow_asset_borrowed_in_flight`
  - `borrow_asset_withdrawal_in_flight`
  - `borrow_asset_paid_to_fees`
  - `collateral_asset_deposited`

## Intentional Behaviors

These are surprising on first read and should not be changed casually. Some are exercised by tests; others are documented policy decisions that should be treated as intentional until changed deliberately:

- Failed borrow finalization intentionally does not roll back fees or yield distribution.
  The borrower temporarily removed liquidity from the available borrow pool during the async receipt window, so charging fees in this case is an anti-griefing policy, not a bug.
  - `templar_common::borrow::BorrowPositionGuard::record_borrow_initial`
  - `templar_common::market::Market::record_borrow_asset_yield_distribution`
  - `templar_common::borrow::BorrowPositionGuard::record_borrow_final`
  Test status:
  - No dedicated regression test currently documents this policy. If behavior around failed borrow finalization changes, add one.
- Repayment is intentionally disallowed while a position is in liquidation.
  - `contract/market/src/impl_helper.rs::Contract::execute_repay`
  - `contract/market/tests/disabled_when_liquidatable.rs::repayment`
- Additional collateral is allowed during liquidation only if it restores the position to a non-liquidatable state.
  - `contract/market/src/impl_helper.rs::Contract::execute_collateralize`
  - `contract/market/tests/disabled_when_liquidatable.rs::disable_collateralize_if_still_liquidatable`
  - `contract/market/tests/disabled_when_liquidatable.rs::allow_sufficient_collateralization_during_liquidation`
- If collateral transfer to the liquidator fails during liquidation, the contract still does not undo the debt reduction or collateral removal.
  This is intentional to avoid griefing liquidations through receiver-side storage unregistration.
  - `contract/market/src/impl_helper.rs::Contract::liquidate_transfer_call_02_finalize`
- `RepayAccount` allows third-party repayment of another account when the position is not liquidatable.

## NEAR-Specific Security Notes

- Be careful with `storage_unregister` interactions for both the market itself and the underlying/collateral token contracts.
- Cross-contract transfer success is only known in finalize callbacks. Never reason about token movement from the initial call alone.
- For borrow-asset transfers, explicitly consider receiver-side storage-registration failure. This is a realistic way to reach the failed-finalize branch for NEP-141 tokens, and the current policy is to restore liquidity while retaining fees.
- Out-of-gas or callback failure can leave partially progressed async flows. Preserve compensation logic in finalize paths.
- `ft_transfer_call` and `mt_transfer` receiver behavior depends on token standards and return-value semantics. Keep `ReturnStyle` handling aligned with the token path being used.
- If you change gas constants in callback chains, re-review receipt sequencing and failure modes.

## Testing Guidance

- Fast shared-logic regression:
  - `cargo test -p templar-common --lib -- --nocapture`
- Market contract tests:
  - `cargo test -p templar-market-contract -- --nocapture`

For `near-workspaces` tests:

- Prefer prebuilt test contracts.
- `./script/test.sh` already runs `./script/prebuild-test-contracts.sh` and sets `TEST_CONTRACTS_PREBUILT=1`.
- If running market integration tests directly, prefer prebuilding first and setting `TEST_CONTRACTS_PREBUILT=1`.
- Narrow proxy-oracle verification command:
  - `TEST_CONTRACTS_PREBUILT=1 cargo test proxy_oracle -p templar-market-contract -- --nocapture`
  - Status: passed locally on 2026-05-05 after `./script/prebuild-test-contracts.sh`

## Documentation Maintenance

- Update this file when market-specific security assumptions, invariants, callback patterns, or required verification steps change.
