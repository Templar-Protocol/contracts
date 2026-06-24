# Vault Curator Guide

This guide explains how a Templar vault is structured and the economic and
governance levers available to a curator: the fees a curator earns and the
configurable "switches" that control risk — together with the timelock rules
that govern when each change takes effect.

## High-level vault structure

A Templar vault is a **single-asset, ERC-4626-style yield vault**. Depositors
supply one underlying token (NEP-141 on NEAR / SEP-41 on Soroban) and receive
transferable **shares**; the curator allocates the pooled assets across a chosen
set of on-chain lending **markets** to earn yield.

**Kernel + executor architecture:**

- `templar-vault-kernel` — chain-agnostic source of truth: state machine, math,
  fee accrual, and invariants. It returns *effects* (mint/burn/transfer/emit)
  rather than touching chain state directly.
- `contract/vault/near` and `contract/vault/soroban` — per-chain executors that
  persist state, enforce authorization, and execute kernel effects.
- `contract/vault/curator-primitives` — shared policy/RBAC/governance helpers
  (caps, cap groups, supply queue, timelocks, restrictions).

**Accounting invariant:** `total_assets = idle_assets + external_assets`.

- `idle_assets` — uninvested cash buffer held by the vault. It also serves as the
  withdrawal liquidity buffer; there is no separate "idle market".
- `external_assets` — principal deployed into markets.

**Flows:**

- **Deposit** → the vault mints shares pro-rata; assets sit idle until allocated.
- **Allocate** (Allocator/keeper) → moves idle assets into markets in
  **supply-queue** order, respecting per-market and cap-group limits.
- **Withdraw** → two-phase, async, keeper-routed: `request_withdraw` escrows the
  shares and starts a cooldown; after the cooldown, `execute_withdraw` pulls
  liquidity from idle first, then from markets along a route, then settles and
  burns the escrowed shares.

**Roles:**

- **Owner** — top-level governance (sets curator/sentinel, fees, timelocks,
  restrictions).
- **Curator** — policy admin: market caps, cap groups, supply queue, and market
  removal. Implicitly holds the Allocator role.
- **Allocator** — operational keeper: allocations, rebalances, withdrawals,
  refreshes.
- **Sentinel** — emergency authority: pause / tighten restrictions immediately,
  abort in-flight operations, and **revoke any pending timelocked change**.

## Curator economics (fees)

There are two fee types. Both are **minted as new shares** to a configurable
recipient (so a recipient must be able to hold shares / be registered to receive
them).

| Fee | Basis | Cap |
|-----|-------|-----|
| **Management** | Time-weighted on AUM (`rate × AUM × elapsed / 1yr`), accrues regardless of performance | **5% / year** |
| **Performance** | AUM **growth** since the last accrual checkpoint; zero on flat or down periods | **50% of profit** |

Rates are WAD-scaled (`1e18 = 100%`). Each fee has its own recipient, and they
can differ.

Two nuances worth understanding:

- **Checkpoint, not all-time high-water mark.** Fees accrue on *every*
  interaction (deposit, withdraw, allocate, fee change), and the fee anchor
  resets to the current AUM each time. Profit is measured as
  `current_AUM − anchor_AUM`. If AUM is flat or down versus the anchor, the
  performance fee is zero — but because the anchor resets downward after a loss,
  a subsequent recovery *is* chargeable. This is "growth since the last
  checkpoint", not "growth above the all-time peak". (Losses are rare in
  over-collateralized lending, but the distinction matters.)
- **Anti-donation cap** (`max_total_assets_growth_rate`, optional). Caps how fast
  AUM is allowed to count for fee accrual:
  `effective_AUM = min(current, last × (1 + max_rate × dt/yr))`. This prevents a
  one-block token donation from inflating fees. Relaxing or removing this cap is
  timelocked.

## Governance switches and the timelock rule

**The principle behind every switch:** changes that **disadvantage depositors**
are **timelocked** (so depositors can exit first, and the Sentinel can veto);
changes that **protect or benefit depositors** take effect **immediately**.

Timelocks are configurable per kind, bounded between **0 and 30 days** (default 2
days).

| Switch | Function | Immediate (depositor-friendly) | Timelocked (depositor-adverse) |
|--------|----------|-------------------------------|-------------------------------|
| **Fees** | `set_fees` | Fee **decrease** | Fee **increase**, any **recipient change**, relaxing the growth cap |
| **Market cap** | `submit_cap` | **Lower** cap (incl. set to 0 = stop deposits) | **Raise** cap, or a **new** market |
| **Cap group** (absolute + relative) | `submit_cap_group_update` | **Tighten** (lower / add a cap) | **Loosen** (raise / remove a cap). Relative cap ≤ 100% |
| **Restrictions** | `set_restrictions` | **Pause / tighten** (blacklist, narrow whitelist) | **Unpause / relax** |
| **Timelock length** | `submit_timelock` | **Lengthen** | **Shorten** (waits under the old, longer timelock) |
| **Market removal** | `submit_market_removal` | — | Always timelocked; requires the cap to already be 0 |

Other levers:

- **Supply queue** (`set_supply_queue`) — the ordered list of allocation targets.
  Up to 64 markets, no duplicates, every market must have a cap > 0, and the
  vault must be Idle.
- **Cap groups** — cluster correlated markets under a shared limit. The effective
  limit is `min(absolute_cap, relative_cap × total_AUM)`.
- **Cooldowns** — withdrawal (default **1 hour**), market refresh (30 seconds),
  idle resync (120 seconds).
- **Abdicate** (`abdicate`) — permanently and irreversibly disable a governance
  method (for example, to lock fees forever).
- **Sentinel revoke** — `revoke_pending_*` cancels any pending timelocked change.

## Worked examples

1. **Raising the performance fee 10% → 20%** is timelocked (e.g. 2 days).
   Depositors see the pending change and can withdraw; the Sentinel can revoke
   it. *Lowering* it 20% → 10% applies instantly.
2. **A market turns risky.** Cutting its cap 1M → 500k USDC is **immediate**;
   setting it to **0** stops new allocations now (the first step of winding it
   down). *Raising* a cap 1M → 2M is timelocked.
3. **Incident response.** `set_restrictions(Paused)` halts the vault
   **immediately**; *un-pausing* is timelocked, so users get notice before
   normal operation resumes.
4. **A cap group "blue-chip"** with an absolute cap of 5M and a relative cap of
   40%: at 8M AUM the cluster can hold at most `min(5M, 0.40 × 8M = 3.2M) = 3.2M`.
   The limit scales with AUM until the 5M ceiling binds.
5. **Fee accrual.** The vault grows 10M → 11M between interactions. With a 20%
   performance fee, roughly 200k (assets-equivalent) of shares mint to the
   performance recipient (20% of the 1M gain); a 2%/yr management fee
   additionally mints time-weighted shares on the 10M base for the elapsed
   interval. Both are dilutive to existing holders by exactly the minted share
   amount.
