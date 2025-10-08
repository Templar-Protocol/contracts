# Templar Vault – Withdrawals

This document explains how withdrawals work in the vault as currently implemented.

Summary
- The vault performs “best-effort now” withdrawals. There is no persistent vault-level withdrawal queue.
- If there is insufficient liquidity across all markets, the vault refunds escrowed shares and stops. No payout is made.
- Partial progress may occur while attempting the withdrawal (some markets may return funds), but payout to the user only happens when the full requested amount is collected.

Entry points
- withdraw(amount, receiver)
  - Convenience wrapper that computes shares = preview_withdraw(amount) and calls redeem(shares, receiver).
- redeem(shares, receiver)
  - Escrows the caller’s shares in the vault and starts a withdrawal operation.

Operational flow
1) Escrow shares
   - redeem(shares) transfers the shares from the caller into the vault (escrow). Shares are not burned yet.

2) Accrue fees and compute targets
   - The vault accrues any pending performance fee, then computes the underlying amount to return for the given shares.

3) Consume idle liquidity first
   - The vault immediately uses idle_balance (underlying already held in the vault) to cover part of the request, if available.

4) Iterate markets in withdraw_queue
   - For the remaining amount, the vault iterates withdraw_queue in order.
   - For each market, it requests and executes a supply withdrawal on that market (create_supply_withdrawal_request + execute_next_supply_withdrawal_request).
   - After execution, the vault reads the market position to reconcile the new principal and determine how much underlying actually came back.

5) Payout on full fulfillment
   - When collected == requested (remaining == 0), the vault transfers underlying to receiver.
   - On transfer success: idle_balance decreases by the payout and escrowed shares are burned.
   - On transfer failure: escrowed shares are returned to the owner; idle_balance remains unchanged.

6) Insufficient liquidity (end of queue)
   - If the vault reaches the end of withdraw_queue with remaining > 0:
     - Escrowed shares are returned to the owner.
     - Any partial funds that did come back from markets remain in the vault’s idle_balance (they are not paid out).
     - The withdrawal operation stops. Callers can retry later.

Events to watch
- RedeemRequested { shares, estimated_assets } – emitted when a withdrawal begins.
- WithdrawalPositionMissing / WithdrawalPositionReadFailed – diagnostics when reading a market position after a withdrawal step fails.
- WithdrawalStopped { remaining, collected, reason } – emitted when the withdrawal stops without completing (e.g., InsufficientLiquidity).
- PayoutStopped / PayoutUnexpectedState – diagnostics for payout errors.
- Note: There is currently no explicit “payout succeeded” event; payout success is the normal completion path.

Design rationale (simplicity)
- No persistent queue in the vault: fewer invariants, fewer public methods, and no long-lived state.
- Users/integrators can pre-check with preview_withdraw(amount) to estimate shares and can retry later if markets are illiquid.

Integrator tips
- Prefer preview_withdraw(amount) to understand the share cost beforehand.
- To reduce the chance of stopping due to illiquidity, withdraw smaller amounts that can be satisfied by idle_balance or by likely-available market liquidity.
- Monitor events:
  - Successful payout: the final callback after_send_to_user returns success (no explicit event).
  - Stopped without payout: WithdrawalStopped will include remaining and collected amounts and a reason.

Future enhancements considered
- Vault-level queued withdrawals (keep shares escrowed and resume later) can improve UX during illiquid periods, but add complexity (queue state, execution/cancel flows, griefing protections). The current implementation intentionally opts for the simpler “best-effort now” model.
