# Templar Vault: Architecture, Codebase, and Flows

This document explains how the vault works end-to-end: roles and permissions, data flow, deposits and withdrawals, and the async allocation/withdraw pipelines.

## High-level overview

- The vault issues shares over an underlying asset and allocates liquidity into configured markets.
- Allocation uses a supply_queue for ordering deposits/idle funds into markets.
- Withdrawals are queue-less (keeper-routed):
  - Order is chosen per withdrawal execution, not stored.
  - A keeper/executor (an off-chain bot) or caller-provided hints picks which markets to tap first, based on live conditions.
  - The contract enforces safety (caps, enabled flags, timelocks) but does not hardcode a single global withdraw order.
- Operations are asynchronous and guarded by a single state machine (OpState):
  - Idle -> Allocating -> Idle
  - Idle -> Withdrawing -> Payout -> Idle
- Performance fees accrue by minting fee shares on growth only.
- Strict invariants ensure safety and correct accounting.

## AUM model

- The vault uses a BalanceSheet model by default.
- Total assets = idle balance + sum of all market principals.
- Accounting is independent of any withdraw order; price only changes when cash actually moves.

## Codebase map

- src/lib.rs
  - Main contract entrypoint and storage. Declares the NEP-141 share token via FungibleToken, Owner, and Rbac derives.
  - Core public API: governance (owner/curator/guardian/timelock), supply_queue setter, allocation entrypoint (allocate), user flows (withdraw/redeem), queue-less withdraw execution (execute_next_withdrawal_request(route), execute_next_market_withdrawal(op_id)), and utility views (totals, previews, conversions).
  - Storage: market configs, supply_queue (only), market_supply, idle_balance, fee config, pending timelocks/guardian, and pending withdrawal FIFO. There is no on-chain global withdraw order.
  - Op state machine (OpState) and orchestration for allocation and withdraw/payout.
- src/impl_callbacks.rs
  - All async callback handlers (after*supply*_, after*create_withdraw_req, after_exec_withdraw*_ and after_send_to_user).
  - Supports deferred market withdrawal execution via execute_next_market_withdrawal(op_id) when deferment is enabled (default).
  - Context guards (ctx_allocating/ctx_withdrawing), market resolvers, reconciliation helpers, and stop_and_exit\* helpers.
  - Gas constants for cross-contract calls (GET*SUPPLY_POSITION_GAS, AFTER*\*\_GAS).
- src/impl_token_receiver.rs
  - NEP-141 token receiver for deposits. Mints shares on correct token; fully refunds on wrong token (see test execute_supply_wrong_token_refunds_full).
  - Updates idle_balance on deposit; allocation remains separate/async.
- src/wad.rs
  - Fixed-point math utilities: mul_div_floor/mul_div_ceil, WAD constants, and compute_fee_shares.
- src/aux.rs
  - Small helpers and shared utilities used across the contract (kept minimal).
- src/tests.rs and src/impl_callbacks.rs tests
  - Invariants and property tests for flows, supply_queue, conversions, queue-less withdrawal routing, and payout correctness.
- templar_common (external crate)
  - Shared types and cross-contract interfaces: BorrowAsset/FungibleAsset, market::ext_market and messages, vault types (Error, Event, OpState, MarketConfiguration, etc.).

## Roles and permissions

Roles are enforced via RBAC. The Curator is also granted the Allocator role at init.

- Owner: full control; can act in place of any role.
- Curator: manages markets and policy (caps/timelocks/enable/disable). Curator is also implicitly granted Allocator.
- Guardian: can revoke/cancel pending governance actions (timelock/guardian changes, etc.).
- Allocator (operational role): allowed to run allocation and withdrawal execution. This is the role your off-chain keeper bot should hold.

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
- Withdraw routing
  - There is no withdraw_queue. Routing is provided per withdrawal execution by the keeper/caller; design your adapter to accurately report positions and withdrawability.
- Safety
  - The vault tolerates failures by stopping/retrying or refunding escrow; design market adapters to fail fast and be re-entrancy safe.

## Key storage and concepts

- MarketConfiguration per market: { cap, enabled, removable_at }
- market_supply[market] = current principal supplied to that market
- idle_balance = underlying tokens held by the vault
- supply_queue (ordered list of market AccountIds) for allocation only
- pending_cap, pending_timelock, pending_guardian with timelock semantics
- pending_withdrawals FIFO queue (id -> {owner, receiver, escrow_shares, expected_assets, requested_at})
- Fee/virtual offsets for conversions:
  - performance_fee (WAD fraction)
  - last_total_assets (fee accrual anchor)
  - virtual_shares, virtual_assets (stability offsets for conversions/previews)

## Conversions and fees

- Views:
  - get_total_assets() = idle + sum(principal across all markets)
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
- Orchestration
  - Allocation uses supply_queue order; withdrawals are keeper-routed using a per-op route and do not rely on a global on-chain order.
  - Weighted allocation mode uses a temporary in-memory plan (plan) for proportional steps.
- Consistent stop behavior
  - Any index/op_id drift or cross-contract error stops the op, reconciles remaining (for allocation), or refunds/parks escrow (for withdrawal), then returns to Idle.

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

## Withdrawal and redeem flow (queue-less, keeper-routed)

Two phases: user requests (escrow) and keeper-routed execution (pull liquidity, pay out).

1. User request (escrow shares)

- withdraw(amount, receiver)
  - Computes shares_needed via preview_withdraw and defers to redeem.
- redeem(shares, receiver)
  - Transfers shares from owner to the vault (escrow) without burning.
  - Converts shares to assets via convert_to_assets (estimated).
  - Emits WithdrawQueued; enqueues pending withdrawal (owner, receiver, escrow_shares, expected_assets).
  - Does NOT start withdrawal; keeper (Allocator) must call execute_next_withdrawal_request(route).

2. Execution by Allocator/keeper (Idle -> Withdrawing -> Payout -> Idle)

- execute_next_withdrawal_request(route: Vec<AccountId>):
  - Pops the next pending withdrawal by id and calls start_withdraw(expected_assets, receiver, owner, escrow_shares) with the provided per-op route.
  - Idle-first: collected = min(idle_balance, amount), remaining = amount - collected.
  - Sets OpState::Withdrawing { index=0, remaining, receiver, collected, owner, escrow_shares }.

- For each market in route:
  - If remaining == 0, skip to payout.
  - If market principal is zero, skip to next.
  - The vault creates a market withdrawal request up to min(remaining, principal) via create_supply_withdrawal_request(...).
  - By default, requests are created with deferment (defer_market_execute = true). The keeper then calls execute_next_market_withdrawal(op_id) to execute created requests (may be called multiple times).
  - After execution, the vault queries get_supply_position(...) and reconciles:
    - credited = min(before - after, remaining)
    - idle_balance += credited
    - remaining -= credited; collected += credited

- Completion/parking:
  - If remaining hits zero, the vault pays the receiver and burns the proportional escrowed shares.
  - If the route is exhausted before need is satisfied, the vault parks the request (escrow remains). The keeper can retry later with a new route.

- Payout finalization (after_send_to_user):
  - On success:
    - idle_balance -= payout_amount
    - Burn only the proportional shares and refund the remainder to the owner.
    - Go Idle.
  - On failure:
    - Refund full escrow to owner; leave idle unchanged; go Idle.

Important

- The route applies only to the current withdrawal op and is not stored. There is no persistent withdraw order on-chain.
- The vault will skip markets with zero principal; it will not exceed principal, and it reconciles actual results after each market call.

## Typical routing policies (off-chain)

- Liquidity-first: withdraw from markets that can return funds immediately (max withdrawable now).
- Cheapest-first: minimize gas/calls or on-market fees.
- Risk-aware: prefer healthiest positions; avoid stressed ones unless necessary.
- Pro-rata: take proportionally from all markets holding principal.
- Round-robin/aging: fairness over time across markets.
- Don’t grow risk: prefer markets with cap=0 (being wound down) before touching growth markets.

## Queues and market management

- set_supply_queue(markets):
  - Requires Idle; rejects duplicates; each market must have cap > 0.
- Note:
  - There is no withdraw_queue. Withdrawals are routed per operation by the keeper/caller.

- submit_cap(market, new_cap), accept_cap(market):
  - Lowering cap applies immediately (and may disable the market if cap == 0).
  - Raising cap is timelocked; accept after timelock.
  - Enabling/disabling does not affect any on-chain withdraw order (there is none).

- submit_market_removal(market), revoke_pending_market_removal(market):
  - Start/stop a removal timelock; actual removal occurs once conditions are met by governance.
- Removing a market
  - Requires cap == 0 and no pending cap raise.
  - If principal > 0: removable_at set via submit_market_removal and timelock elapsed.
  - Removing a market deletes its configuration but does not clear market_supply; total assets continue to include remaining principal until withdrawn.

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
  - Allocator: execute_next_withdrawal_request(route), execute_next_market_withdrawal(op_id)
- Governance:
  - Owner/Curator/Guardian as listed above.

## API changes (for integrators/keepers)

- execute_next_withdrawal_request now requires a route: Vec<AccountId> (ordered preference for this withdrawal).
- allocator_execute_next_market_withdrawal(op_id) executes the next created market request when deferment is enabled (default).
- Curator is granted Allocator by default at initialization; keepers must use an account that has the Allocator role (or be the Curator/Owner).

## Error handling and stop semantics

- Allocation
  - Any transfer/position read error or state mismatch stops the operation, returns remaining to idle, clears plan, and returns to Idle.
- Withdrawal
  - Any state mismatch or market call failure advances to the next market; reaching end-of-route parks the request for later retries or triggers payout-if-collected.
- Payout
  - On success: burn proportional escrow and refund the rest; on failure: refund full escrow; in both cases the vault returns to Idle.
- All stop paths emit structured events for indexing and debugging.

## Key invariants

- Single op in flight; ensure_idle() on all mutating entrypoints.
- No global withdraw order is stored on-chain; withdrawals are routed per execution.
- Allocation reservation never exceeds idle or available cap (clamp_allocation_total).
- Payout success always reduces idle by paid amount and burns only proportional escrow.
- Fees mint only on positive growth.

## Testing and local development

- Unit/property tests cover:
  - Cap/timelock rules and market removal.
  - Allocation pipeline, queue-less withdraw routing, payout success/failure, and escrow settlement math.
  - Fee accrual on growth only, and conversion/preview bounds with virtual offsets.
  - Token receiver behavior (wrong token refund).
- Running tests:
  - cargo test -p templar-vault
- Tips:
  - When integrating a new market, first wire get_supply_position and dry-run the withdraw path with a short route to validate reconciliation.

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
  - If enabling (cap > 0): no extra storage for withdraw order (none exists).
- set_supply_queue(markets)
  - Storage for markets added that were not previously in the queue.
- allocate(weights, amount)
  - No storage deposit for withdraw routing (route is ephemeral and provided per execution).
- withdraw/redeem
  - PendingWithdrawal queue entry per request (escrowed shares are held until payout/refund).

Refund policy

- For simplicity and in line with many Ethereum contracts, we do not refund storage on removals (e.g.,
  queue removals, consumed pending withdrawals, deleted configs). This avoids complexity and edge cases
  around attribution.
