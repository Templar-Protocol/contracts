# Templar Vault: Architecture, Codebase, and Flows

This document explains how the vault works end-to-end: roles and permissions, data flow, deposits and withdrawals, and the async allocation/withdraw pipelines.

## High-level overview

- The vault issues shares over an underlying asset and allocates liquidity into configured markets.
- Two ordered queues drive behavior:
  - supply_queue: allocation order for deposits/idle funds to be supplied to markets.
  - withdraw_queue: priority order to pull liquidity back from markets.
- Operations are asynchronous and guarded by a single state machine (OpState):
  - Idle -> Allocating -> Idle
  - Idle -> Withdrawing -> Payout -> Idle
- Performance fees accrue by minting fee shares on growth only.
- Strict invariants ensure queue correctness and safe removal of markets.

## Codebase map

- src/lib.rs
  - Main contract entrypoint and storage. Declares the NEP-141 share token via FungibleToken, Owner, and Rbac derives.
  - Core public API: governance (owner/curator/guardian/timelock), queue setters, allocation entrypoint (allocate), user flows (withdraw/redeem), and utility views (totals, previews, conversions).
  - Storage: market configs, queues, market_supply, idle_balance, fee config, pending timelocks/guardian, and pending withdrawal FIFO.
  - Op state machine (OpState) and orchestration for allocation and withdraw/payout.
- src/impl_callbacks.rs
  - All async callback handlers (after_supply_*, after_create_withdraw_req, after_exec_withdraw_* and after_send_to_user).
  - Context guards (ctx_allocating/ctx_withdrawing), market resolvers, reconciliation helpers, and stop_and_exit* helpers.
  - Gas constants for cross-contract calls (GET_SUPPLY_POSITION_GAS, AFTER_*_GAS).
- src/impl_token_receiver.rs
  - NEP-141 token receiver for deposits. Mints shares on correct token; fully refunds on wrong token (see test execute_supply_wrong_token_refunds_full).
  - Updates idle_balance on deposit; allocation remains separate/async.
- src/wad.rs
  - Fixed-point math utilities: mul_div_floor/mul_div_ceil, WAD constants, and compute_fee_shares.
- src/aux.rs
  - Small helpers and shared utilities used across the contract (kept minimal).
- src/tests.rs and src/impl_callbacks.rs tests
  - Invariants and property tests for flows, queues, conversions, and payout correctness.
- templar_common (external crate)
  - Shared types and cross-contract interfaces: BorrowAsset/FungibleAsset, market::ext_market and messages, vault types (Error, Event, OpState, MarketConfiguration, etc.).

## Roles and permissions

Roles are enforced via RBAC. The Curator is also granted the Allocator role at init.

- Owner
  - set_curator(account)
  - set_is_allocator(account, allowed)
  - submit_guardian(new_g), accept_guardian(), revoke_pending_guardian()
  - submit_timelock(seconds), accept_timelock(), revoke_pending_timelock()
  - set_fee_recipient(account), set_performance_fee(fee)
  - set_skim_recipient(account), skim(token)
- Curator (Curator also has Allocator)
  - submit_cap(market, new_cap), accept_cap(market), revoke_pending_cap(market)
  - submit_market_removal(market), revoke_pending_market_removal(market)
- Allocator
  - set_supply_queue(markets)
  - set_withdraw_queue(markets)
  - allocate(weights, amount)
  - execute_next_withdrawal_request()
- Guardian
  - revoke_pending_timelock()

Note
- All mutating ops require the vault to be Idle (single-op-at-a-time). Methods enforce this via ensure_idle().

## External integrations and interfaces

- Underlying token (NEP-141)
  - The vault is a NEP-141 receiver. Users deposit via ft_transfer_call to the vault; only the configured underlying token is accepted.
  - On correct token: the vault mints shares and increases idle_balance.
  - On wrong token: the vault refunds in full and mints no shares.
- Market adapters
  - Allocation to markets uses underlying_asset.transfer_call(..., DepositMsg::Supply).
  - Withdrawals use the market interface:
    - create_supply_withdrawal_request(BorrowAssetAmount)
    - execute_next_supply_withdrawal_request()
    - get_supply_position(vault_id) to verify changes and reconcile accounting.
- Gas model
  - Cross-contract calls use fixed gas budgets:
    - AFTER_SUPPLY_ENSURE_GAS, GET_SUPPLY_POSITION_GAS, AFTER_SUPPLY_POSITION_CHECK_GAS
    - AFTER_CREATE_WITHDRAW_REQ_GAS, AFTER_SEND_TO_USER_GAS
  - On any callback mismatch or failure, the operation gracefully stops and reverts to Idle with safe reconciliation.

## Integrating a new market

- Required market endpoints (templar_common::market::ext_market)
  - get_supply_position(vault_id) -> SupplyPosition
  - create_supply_withdrawal_request(BorrowAssetAmount)
  - execute_next_supply_withdrawal_request()
- Deposit message and units
  - Underlying allocation uses DepositMsg::Supply with underlying units.
- Queue membership
  - Ensure the market is in withdraw_queue whenever principal > 0; the vault also enforces this on its own after allocation steps.
- Safety
  - The vault tolerates failures by stopping/retrying or refunding escrow; design market adapters to fail fast and re-entrant safe.

## Key storage and concepts

- MarketConfiguration per market: { cap, enabled, removable_at }
- market_supply[market] = current principal supplied to that market
- idle_balance = underlying tokens held by the vault
- supply_queue and withdraw_queue (ordered lists of market AccountIds)
- pending_cap, pending_timelock, pending_guardian with timelock semantics
- pending_withdrawals FIFO queue (id -> {owner, receiver, escrow_shares, expected_assets, requested_at})
- Fee/virtual offsets for conversions:
  - performance_fee (WAD fraction)
  - last_total_assets (fee accrual anchor)
  - virtual_shares, virtual_assets (stability offsets for conversions/previews)

## Conversions and fees

- Views:
  - get_total_assets() = idle + sum(principal across withdraw_queue markets)
  - get_total_supply()
  - get_max_deposit() aggregates per-market remaining caps in supply_queue order
  - convert_to_shares(assets), convert_to_assets(shares)
  - preview_deposit/mint/withdraw/redeem
- Fees:
  - internal_accrue_fee() mints fee shares only on growth (current_total_assets > last_total_assets).
  - Conversions simulate fee accrual and include virtual offsets via compute_effective_totals.

- Effective totals
  - All previews and conversions simulate fee accrual first and apply virtual_shares and virtual_assets to stabilize edge cases at low supply/assets.
- Accrual policy
  - internal_accrue_fee() mints fee shares only when get_total_assets() > last_total_assets (no fees on losses or flat performance).
  - Fee rate is a WAD fraction and bounded; fee_recipient changes first accrue under the old recipient.

## Execution model at a glance

- Single-operation state machine, enforced by ensure_idle() on all mutating entrypoints:
  - Idle -> Allocating -> Idle
  - Idle -> Withdrawing -> Payout -> Idle
- Queue-driven orchestration
  - supply_queue defines allocation order; withdraw_queue defines liquidity pull priority.
  - Weighted allocation mode uses a temporary in-memory plan (plan) for proportional steps.
- Consistent stop behavior
  - Any index/op_id drift or cross-contract error stops the op, reconciles remaining (for allocation), or refunds escrow (for withdrawal), then returns to Idle.

## Deposit and mint flow

User deposits underlying and receives vault shares. Allocation into markets is separate.

- User interface:
  - Preview: preview_deposit(assets) -> expected shares
  - Convert: convert_to_shares
  - Mint preview: preview_mint(shares)

- Actual deposit:
  - The vault expects to receive the underlying via NEP-141 transfer (see token receiver).
  - If an unexpected token sends funds, the vault refunds fully (see test execute_supply_wrong_token_refunds_full).

- Post-deposit state:
  - idle_balance increases
  - No automatic allocation: allocation is triggered by Allocator via allocate(...)

- Token receiver path
  - Accept only the configured underlying token. Wrong-token deposits are refunded 100%.
  - On success: idle_balance += assets; shares minted according to convert_to_shares (fee- and virtual-offset-aware).
- No auto-allocation
  - Deposits remain idle until an Allocator triggers allocate(...).

## Allocation pipeline (Idle -> Allocating -> Idle)

Triggered by Allocator:
- allocate(weights=[], amount=None)
  - Queue-based if weights empty; weighted if provided.
  - total reserved = clamp_allocation_total(requested or idle), subject to get_max_deposit().
  - start_allocation(total) reserves from idle (idle_balance -= total), sets OpState::Allocating { remaining=total, index=0 }, emits AllocationStarted.

Async loop (step_allocation):
- Picks the next market from plan (weighted) or supply_queue (queue-based).
- Computes room and to_supply, emits AllocationStepPlanned.
- If to_supply == 0, skips and advances index.
- Else transfers underlying to market via transfer_call(..., DepositMsg::Supply) and awaits after_supply_1_check.

Callbacks:
- after_supply_1_check:
  - Validates current op and resolves market.
  - If transfer failed, stops and returns remaining back to idle (stop_and_exit_allocating).
  - Else reads position via get_supply_position(...) -> after_supply_2_read.
- after_supply_2_read:
  - Reads new_principal, computes accepted_event = new_principal - before.
  - Updates market_supply, emits AllocationStepSettled.
  - Ensures market is in withdraw_queue if principal > 0.
  - Advances index and remaining; loops or exits.

Exit:
- stop_and_exit_allocating(None) emits AllocationCompleted and returns any remaining to idle.
- Any error stops, returns remaining to idle, clears plan, and goes Idle.

- Weighted vs queue-based
  - If weights are provided, per-step targets are proportional to remaining and residual weights; the last market takes the remainder.
  - If no weights, the vault allocates in supply_queue order, up to room (cap - current principal).
- Reservation and reconciliation
  - start_allocation reserves only the planned amount (idle_balance -= amount).
  - On completion or on any failure, remaining is returned to idle_balance.
- Market (re)inclusion
  - If a market’s principal becomes > 0, it is ensured to be present in withdraw_queue. Re-including a market with pre-existing principal adjusts last_total_assets to avoid fee-on-reinclude.

## Withdrawal and redeem flow

Two phases: user requests (escrow) and allocator executes (pull liquidity, pay out).

1) User request (escrow shares)

- withdraw(amount, receiver)
  - Computes shares_needed via preview_withdraw and defers to redeem.
- redeem(shares, receiver)
  - Transfers shares from owner to the vault (escrow) without burning.
  - Converts shares to assets via convert_to_assets (estimated).
  - Emits WithdrawQueued; enqueues pending withdrawal (owner, receiver, escrow_shares, expected_assets).
  - Does NOT start withdrawal; allocator must call execute_next_withdrawal_request().

2) Allocator executes (Idle -> Withdrawing -> Payout -> Idle)

- execute_next_withdrawal_request():
  - Pops the next pending withdrawal by id and calls start_withdraw(expected_assets, receiver, owner, escrow_shares).

start_withdraw:
- Uses idle-first: collected = min(idle_balance, amount), remaining = amount - collected.
- Sets OpState::Withdrawing { index=0, remaining, receiver, collected, owner, escrow_shares }.

step_withdraw:
- If remaining == 0:
  - Switches to OpState::Payout and transfers collected to receiver; after_send_to_user burns escrow proportionally and refunds unused escrow.
- Else:
  - Iterates withdraw_queue[index]:
    - If market principal is zero, skip (advance index).
    - Else create_supply_withdrawal_request(to_request) -> after_create_withdraw_req -> execute_next_supply_withdrawal_request() -> after_exec_withdraw_req -> read position -> after_exec_withdraw_read.

Callbacks:
- after_create_withdraw_req:
  - On failure: advance index; if end-of-queue, transition to Payout/Refund based on collected.
- after_exec_withdraw_req:
  - Reads position afterwards to verify change.
- after_exec_withdraw_read:
  - Computes credited and updates:
    - credited = min(before - new, need), remaining_next = rem - credited, collected_next = coll + credited, idle += credited.
  - If remaining_next == 0:
    - If collected_next > 0 => Payout
    - Else refund full escrow and go Idle.
  - Else advance to next market and continue.

Payout finalization:
- after_send_to_user:
  - On success:
    - idle_balance -= payout_amount
    - Burn only the proportional shares and refund the remainder:
      - burn_shares = compute_burn_shares(escrow_shares, collected, requested_total)
      - (to_burn, refund_shares) = compute_escrow_settlement(escrow_shares, burn_shares)
      - burn to_burn from escrow; transfer refund_shares back to owner
    - Go Idle.
  - On failure:
    - Refund full escrow to owner; leave idle unchanged; go Idle.

Stop behavior:
- Any callback receiving stale op_id or mismatched index will gracefully stop the op, refunding escrow (for withdraw) or reconciling remaining (for allocation), and return to Idle.

- Two-phase withdrawal
  - User redeem/withdraw: shares are escrowed in the vault account (not burned yet) and a pending withdrawal is queued with an expected_assets estimate.
  - Operator execute_next_withdrawal_request(): drives the async pipeline to collect assets and pay out.
- Idle-first payout
  - The vault first uses idle_balance. Any remaining amount is pulled from markets in withdraw_queue order.
- On-market withdrawal
  - For each market: create request, execute next request, then read position to verify principal reduction. Credited amounts increase idle_balance.
- Payout finalization
  - On success: idle_balance -= paid amount; burn only the proportional fraction of escrow_shares corresponding to the paid fraction; refund remaining escrow to the owner.
  - On failure: refund full escrow; idle_balance unchanged.

## Queues and market management

- set_supply_queue(markets):
  - Requires Idle; rejects duplicates; each market must have cap > 0.
- set_withdraw_queue(queue):
  - Requires Idle; rejects duplicates; every enabled or holding market must be present.
  - Removing a market requires:
    - cap == 0
    - no pending cap change
    - if principal > 0: removable_at set and timelock elapsed
  - Removing a market also removes its configuration.

- submit_cap(market, new_cap), accept_cap(market):
  - Lowering cap applies immediately (and may disable the market if cap == 0).
  - Raising cap is timelocked; accept after timelock.
  - Enabling a market ensures it’s present in withdraw_queue.

- submit_market_removal(market), revoke_pending_market_removal(market):
  - Start/stop a removal timelock; actual removal occurs via set_withdraw_queue.

- Before removing a market from withdraw_queue:
  - cap == 0 and no pending cap raise.
  - If principal > 0: removable_at set via submit_market_removal and timelock elapsed.
- Removing a market deletes its configuration but does not clear market_supply; off-queue principal is intentionally ignored by get_total_assets().

## Fee policy

- set_performance_fee(fee) sets the WAD fraction (capped; fees accrue only on profits).
- internal_accrue_fee() mints fee shares to fee_recipient and updates last_total_assets.
- Conversions use compute_effective_totals to simulate fee shares and apply virtual offsets.

## Reference: primary external methods by role

- Deposits:
  - User: ft_transfer_call to the vault (see token receiver), or application-level front-end wraps this.
- Allocation:
  - Allocator: allocate(weights, amount)
- Withdrawals:
  - User: redeem(shares, receiver) or withdraw(amount, receiver)
  - Allocator: execute_next_withdrawal_request()
- Governance:
  - Owner/Curator/Guardian as listed above.

## Error handling and stop semantics

- Allocation
  - Any transfer/position read error or state mismatch stops the operation, returns remaining to idle, clears plan, and returns to Idle.
- Withdrawal
  - Any state mismatch or market call failure advances to the next market; reaching end-of-queue triggers payout-if-collected or escrow refund.
- Payout
  - On success: burn proportional escrow and refund the rest; on failure: refund full escrow; in both cases the vault returns to Idle.
- All stop paths emit structured events for indexing and debugging.

## Key invariants

- Single op in flight; ensure_idle() on all mutating entrypoints.
- Withdraw queue must contain every enabled or holding market.
- Allocation reservation never exceeds idle or available cap (clamp_allocation_total).
- Payout success always reduces idle by paid amount and burns only proportional escrow.
- Fees mint only on positive growth.

## Testing and local development

- Unit/property tests cover:
  - Queue invariants, cap/timelock and market removal rules.
  - Allocation/withdraw pipelines, payout success/failure, and escrow settlement math.
  - Fee accrual on growth only, and conversion/preview bounds with virtual offsets.
  - Token receiver behavior (wrong token refund).
- Running tests:
  - cargo test -p templar-vault
- Tips:
  - When integrating a new market, first wire get_supply_position and dry-run the withdraw path to validate reconciliation.

## Storage management

This vault uses a per-entry storage charging model. Callers attach deposits only when their action may
create new storage entries. We size entries conservatively using AccountId::MAX_LEN and fixed field sizes,
to avoid relying on runtime storage usage “diffs”.

What the contract pays for
- RBAC storage: role membership (Owner/RBAC lists) is paid by the contract. Callers are not charged
storage deposits for set_curator, set_is_allocator, or guardian role changes.

Conservative sizing
- AccountId bytes are charged at MAX_LEN to keep pricing simple and deterministic.
- Map/queue overheads are charged with fixed constants.
- PendingWithdrawal size is a fixed upper bound of its fields.

When a deposit is required
- submit_cap(market, new_cap)
  - If market is new: config entry + market_supply entry.
  - If raising cap above current: pending_cap entry.
- accept_cap(market)
  - If enabling (cap > 0) and the market is not in withdraw_queue: 1 queue slot.
- set_supply_queue(markets)
  - Storage for markets added that were not previously in the queue.
- set_withdraw_queue(queue)
  - Storage for markets added that were not previously in the queue.
- allocate(weights, amount)
  - Up-front deposit to cover potential withdraw_queue insertions for any candidate market in the
allocation run (supply_queue for queue mode; weighted plan markets for weighted mode).
- withdraw/redeem
  - PendingWithdrawal queue entry per request (escrowed shares are held until payout/refund).

Refund policy
- For simplicity and in line with many Ethereum contracts, we do not refund storage on removals (e.g.,
queue removals, consumed pending withdrawals, deleted configs). This avoids complexity and edge cases
around attribution. 


