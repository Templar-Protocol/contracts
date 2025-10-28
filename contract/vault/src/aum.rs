use super::{Contract, MarketConfiguration, U128, env, near, require};

/// AUM (Assets Under Management) module
///
/// This module encodes two coherent accounting models for a vault:
/// - GovernanceAbandonment (MetaMorpho-style): AUM counts only markets that are currently
///   "active" in the withdraw_queue. Governance may perform a timelocked write-down by
///   removing a market from the withdraw_queue even if principal remains. Later, governance
///   may perform a timelocked write-up by re-adding that market. Pricing reflects only the
///   set of markets governance currently stands behind.
/// - BalanceSheet (strict accounting): AUM includes every position that still belongs to
///   the vault until assets actually move (principal decreases or funds are paid out).
///   Queue membership is an operational detail; it must not change AUM. Markets cannot be
///   removed from the withdraw_queue while principal > 0.
///
/// Choose exactly one model and apply it consistently across:
/// - How total assets are computed (get_total_assets),
/// - When markets may be omitted from the withdraw queue (policy_removal),
/// - How last_total_assets is adjusted around write-down/write-up boundaries (paper_aum_undercounting),
/// - How fees are minted and previews are computed.
///
/// DO NOT mix semantics (e.g., queue-scoped AUM + removal blocked while principal > 0),
/// as that creates mispricing and attack surface.
///
/// High-level tradeoffs
/// 1) Pricing integrity for mints/redeems
///    - GovernanceAbandonment: Prices reflect only active/supported markets. New depositors
///      are shielded from legacy/stuck risk. Governance actions create price jumps independent
///      of cashflows (write-down/write-up).
///    - BalanceSheet: Price moves only with actual cashflows. No policy-driven price jumps.
///      New depositors buy exposure to legacy risk unless deposits are gated/paused.
///
/// 2) Fee accrual correctness
///    - GovernanceAbandonment: Must protect against spurious fee mint/burn caused by
///      write-down/write-up reclassification. Common pattern: mint fees only on positive delta
///      and bump last_total_assets when re-adding a previously written-down market that still
///      holds principal (see paper_aum_undercounting).
///    - BalanceSheet: Fees accrue strictly on realized growth. No special handling around
///      policy events. Simpler reasoning.
///
/// 3) Cohort fairness (who bears losses/who captures recovery)
///    - GovernanceAbandonment: Existing holders bear loss at the write-down cutover.
///      Post-write-down entrants can capture recovery when/if the market is re-added.
///      Timelocks + events are the fairness mechanism.
///    - BalanceSheet: Losses/recovery remain within the continuous cohort. No cohort transfer
///      at policy boundaries; new depositors buy the bag unless you gate deposits.
///
/// 4) Manipulation/attack surface
///    - GovernanceAbandonment: Potential "yo-yo" price via queue changes. Mitigated by
///      meaningful timelocks, public events, and possibly deposit pauses around effective times.
///      Be explicit about timelock durations and eventing.
///    - BalanceSheet: Risk of "optimistic NAV" if operators keep distressed positions in AUM
///      while accepting deposits. Mitigate by pausing/capping deposits and surfacing a
///      "distressed fraction" metric.
///
/// 5) Liquidity realism in previews
///    - GovernanceAbandonment: Previews align with supported/active markets. Recovery later
///      causes price discontinuity when re-added.
///    - BalanceSheet: NAV reflects all claims, but previews for withdraw may overstate immediacy.
///      Provide a liquidity-aware maxWithdraw estimator along the queue for UIs/policy.
///
/// 6) Operational UX and liveness
///    - GovernanceAbandonment: Clean lever to amputate toxic limbs and keep the product usable
///      for new money. Requires disciplined governance, communications, and auditable events.
///    - BalanceSheet: No governance-driven price shocks, but product can feel "stuck" if assets
///      are illiquid. UIs must handle long-running withdrawals gracefully.
///
/// 7) Complexity
///    - GovernanceAbandonment: More policy code (timelocks, queue-scoped AUM, last_total_assets
///      adjustments, explicit events).
///    - BalanceSheet: Simpler accounting; needs better liquidity simulation and deposit gating.
///
/// Numeric example (to reason about share effects; do not execute)
/// - t0: AUM = 1,000 (100 idle + 900 in Market M), totalSupply = 1,000 shares, price = 1.00.
/// - Model GovernanceAbandonment:
///   t1 write-down: remove M (timelock elapsed), AUM -> 100, price -> 0.10. Existing holders internalize loss.
///   t2 deposit 100: mints 1,000 shares (NAV 100), totalSupply = 2,000.
///   t3 write-up/recovery: re-add M after timelock and bump last_total_assets; AUM -> 1,100, price -> 0.55.
///   Post t1 entrants capture part of recovery. This is intentional under this model.
/// - Model BalanceSheet:
///   t1 acknowledge distress but keep M in AUM. Price stays 1.00.
///   t2 deposit 100: mints 100 shares. New depositors buy distressed exposure.
///   t3 recovery only changes AUM if cash actually moves; no policy jump.
///
/// Eventing (strongly recommended)
/// - Emit MarketWriteDown(id, principal_at_cutover, when) on removal with principal > 0.
/// - Emit MarketWriteUp(id, principal_at_readd, when) on re-add of a market with principal > 0.
/// - Emit WithdrawQueueUpdated, CapChanged, PendingCapAccepted.
/// Clear, auditable events are essential for both fairness and downstream analytics.
///
/// Guardrails per model (must-have)
/// - GovernanceAbandonment:
///   * total assets = sum over withdraw_queue only.
///   * allow omission from queue if cap == 0, no pending cap, and (if principal > 0) removable_at set and elapsed.
///   * on re-add of a market that still holds principal, bump last_total_assets by the principal
///     to avoid accidental fee minting.
///   * consider short deposit pauses around write-down/write-up effective times.
/// - BalanceSheet:
///   * total assets = idle + sum principal across all markets (independent of queue).
///   * cannot remove from queue while principal > 0 (timelock is necessary but not sufficient).
///   * publish staged/receivable metrics for ops visibility; do not feed them into pricing.
///   * implement liquidity-aware maxWithdraw/maxRedeem simulators.
///
/// Testing checklist
/// - Write-down with principal > 0:
///   * GovernanceAbandonment: price drops, no fee minted on loss, re-add bump adjusts last_total_assets.
///   * BalanceSheet: removal blocked; price unchanged until cash moves.
/// - Re-add with principal > 0:
///   * GovernanceAbandonment: last_total_assets bump prevents fee mint; price jumps as intended.
///   * BalanceSheet: re-add is a no-op for accounting; price continuous.
/// - Deposit/withdraw previews across cutovers: no reentrancy or preview mispricing.
/// - Timelock enforcement: cannot write-down or write-up without elapsed timelock.
/// - Attack simulations: attempt to yo-yo the queue within timelocks; ensure protections hold.
///
/// Migration notes
/// - Changing models after deployment is a breaking policy change. If unavoidable, perform with
///   long lead-time, explicit events, and optionally paused deposits during the switchover.
///
/// Terminology
/// - "Staged"/"Primed": operational intent to withdraw; does not change AUM by itself.
/// - "Write-down": governance removal from queue (GovernanceAbandonment) => AUM exclusion.
/// - "Write-up": governance re-add to queue (GovernanceAbandonment) => AUM inclusion with last_total_assets bump.
/// AUM model selector.
///
/// GovernanceAbandonment (MetaMorpho-style):
/// - AUM is withdraw_queue-scoped by design: if a market is not in the withdraw_queue,
///   it does not contribute to AUM.
/// - Removing a market with non-zero principal is allowed, but only after a timelock:
///   cap == 0, no pending cap, removable_at set, and block time >= removable_at.
/// - Effect: governance "writes down" that position for AUM purposes even if tokens
///   remain or recovery is possible.
///
/// BalanceSheet (strict accounting):
/// - AUM includes all positions that still belong to the vault until assets actually
///   move. Queue membership must not change accounting.
/// - Markets cannot be removed from the withdraw_queue while principal > 0.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone)]
pub enum AUM {
    /// GovernanceAbandonment: queue = truth for AUM. See module docs for tradeoffs.
    GovernanceAbandonment,
    /// BalanceSheet: balance sheet = truth for AUM. See module docs for tradeoffs.
    BalanceSheet,
}

impl AUM {
    /// Compute total assets according to the selected AUM model.
    ///
    /// Invariants and expectations:
    /// - `GovernanceAbandonment`:
    ///   * Sums over `withdraw_queue` only. This is an intentional filter; it encodes
    ///     governance's current support set and excludes written-down markets.
    ///   * If you re-add a market that still holds principal, you must pair this with
    ///     a `last_total_assets` bump elsewhere (see `paper_aum_undercounting`) to avoid
    ///     spurious fee minting on reclassification.
    ///
    /// - `BalanceSheet`:
    ///   * Sums over all markets that still have principal. Here we assume `supply_queue`
    ///     enumerates all configured/held markets. If it does not, replace with an
    ///     iteration over the authoritative positions map (e.g., `config` or `positions`).
    ///   * AUM changes only when principal changes or idle balance changes.
    pub fn get_total_assets(&self, c: &Contract) -> U128 {
        U128(match self {
            AUM::GovernanceAbandonment => {
                c.withdraw_queue.iter().fold(c.idle_balance, |prev, m| {
                    prev.saturating_add(c.principal_of(m))
                })
            }
            AUM::BalanceSheet => c
                .market_supply
                .iter()
                .fold(c.idle_balance, |prev, (_, p)| prev.saturating_add(*p)),
        })
    }

    /// Enforce removal policy for omitting a market from the `withdraw_queue`.
    ///
    /// This function should be called at the point where an operator attempts to
    /// remove a market from the `withdraw_queue`. It enforces model-specific invariants.
    ///
    /// - `GovernanceAbandonment`:
    ///   * If the market still has principal, removal requires that a removal timelock
    ///     was scheduled (`removable_at` > 0) and has elapsed (now >= `removable_at`).
    ///   * Additional guards external to this function are typically required:
    ///     cap == 0 and no pending cap. Enforce those where caps are managed.
    ///
    /// - `BalanceSheet`:
    ///   * Removal is prohibited while any principal remains (> 0).
    ///   * Passing a timelock is necessary but not sufficient; ownership hasn't changed.
    pub fn policy_removal(&self, cfg: &MarketConfiguration, has_supply: &bool) {
        match self {
            AUM::GovernanceAbandonment => {
                if *has_supply {
                    require!(
                        cfg.removable_at > 0,
                        "Policy violation: Market still has supply but no removal scheduled"
                    );
                    require!(
                        env::block_timestamp() >= cfg.removable_at,
                        "Policy violation: Removal timelock not elapsed for market"
                    );
                }
            }
            AUM::BalanceSheet => require!(!has_supply, "Policy violation: Supply shares exist"),
        }
    }

    /// Handle accounting around potential AUM undercounting when re-adding markets.
    ///
    /// Context:
    /// - Under `GovernanceAbandonment`, a market removed from the `withdraw_queue` is excluded
    ///   from AUM even if it still holds principal. When that market is later re-added,
    ///   its principal "reappears" in reported AUM. To prevent accidental performance fee
    ///   minting due purely to reclassification (not economic gain), we bump `last_total_assets`
    ///   by the previously excluded principal at re-add time.
    ///
    /// - Under `BalanceSheet`, AUM was never reduced during removal attempts, so no bump is
    ///   necessary. Fees accrue naturally on realized growth only.
    ///
    /// Safety notes:
    /// - Only add `before_principal` that was actually excluded by the prior write-down.
    /// - This adjustment assumes your fee module mints fees on positive delta of
    ///   (`current_total_assets` - `last_total_assets`). If your fee policy differs, audit this path.
    pub fn paper_aum_undercounting(&self, c: &mut Contract, before_principal: &u128) {
        match self {
            AUM::GovernanceAbandonment => {
                if *before_principal > 0 {
                    c.last_total_assets = c.last_total_assets.saturating_add(*before_principal);
                }
            }
            AUM::BalanceSheet => {}
        }
    }
}
