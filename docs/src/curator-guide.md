# Soroban Vault Curator Guide

This guide is for curators operating a Templar **Soroban (Stellar)** vault. It
covers how the vault is structured, the fee economics a curator earns, the
configurable risk "switches" and their timelock rules, and the tooling — the
vault CLI/SDK and the curated-vault frontend — used to drive day-to-day
operations.

> **Source & architecture**
>
> - Vault architecture and code (kernel + executors):
>   <https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/README.md>
> - Soroban runtime specifics:
>   <https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/soroban/README.md>
> - Vault CLI / client SDK:
>   <https://github.com/Templar-Protocol/contracts/blob/dev/client/vault/README.md>
> - Curated-vault frontend (UI): <https://app.templarfi.org/vaults/curator/>

## High-level vault structure

A Templar vault is a **single-asset, ERC-4626-style yield vault**. Depositors
supply one underlying token and receive transferable **shares** (a SEP-41 token
on Soroban); the curator allocates the pooled assets across a chosen set of
on-chain lending **markets** to earn yield.

**Kernel + executor architecture** (see the [architecture
README](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/README.md)):

- `templar-vault-kernel` — chain-agnostic source of truth: state machine, math,
  fee accrual, and invariants. It returns *effects* (mint/burn/transfer/emit)
  rather than touching chain state directly.
- `contract/vault/soroban` — the Soroban executor (`CuratorVault<S, A, E>`): it
  loads versioned state, enforces RBAC via `require_auth()` + the shared
  `ActionKind` policy, applies a kernel action, and executes the resulting
  effects against the SEP-41 share token and the underlying asset token.
- `contract/vault/curator-primitives` — shared policy/RBAC/governance helpers
  (caps, cap groups, supply queue, timelocks, restrictions).

**Accounting invariant:** `total_assets = idle_assets + external_assets`.

- `idle_assets` — uninvested cash buffer held by the vault, and the liquidity
  buffer for atomic withdrawals.
- `external_assets` — principal deployed into markets via adapters.

**Markets are adapters.** On Soroban, markets are reached through adapter
contracts that must be **allow-listed and added to the supply queue through
governance before allocation**:

- **Blend adapter** (`contract/vault/soroban/blend-adapter`) — integrates the
  Blend lending protocol.
- **Custodial adapter** (`contract/vault/soroban/custodial-adapter`) — an
  offchain-managed route that forwards assets to a configured custodian/multisig.
  Its NAV is *reported* accounting; treat the custodian and its offchain process
  as part of the vault's trust boundary.

**Roles:**

- **Owner / Governance** — top-level governance (curator/sentinel assignment,
  fees, timelocks, restrictions). On Soroban, proposal submission, timelocks, and
  approvals live in a dedicated **governance contract**.
- **Curator** — policy admin: market caps, cap groups, supply queue, market
  removal. Implicitly holds the Allocator role.
- **Allocator** — operational keeper: allocations, rebalances, withdrawal
  execution, refreshes.
- **Sentinel** — emergency authority: pause / tighten restrictions immediately,
  abort in-flight operations, and revoke pending timelocked changes. The Sentinel
  is a **separate emergency role** from the governance contract.

## Soroban specifics every curator should know

**Two withdrawal modes** (see the [Soroban
README](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/soroban/README.md#soroban-specific-withdrawal-path)):

- `withdraw` / `redeem` — ERC-4626-style **atomic exits from idle liquidity
  only**. They never enqueue work and fail if the requested assets exceed
  `idle_assets`. So `maxWithdraw` / `maxRedeem` can read `0` even when a holder's
  shares are backed by market-deployed assets.
- `request_withdraw` + `execute_withdraw` — the **async** path for positions that
  need allocator/keeper work. `request_withdraw` escrows shares and locks in a
  fixed `expected_assets` claim at request time; `execute_withdraw` (an
  **allocator** action, not a public user exit) settles the queue head only when
  it is cooled down and fully covered by idle assets, otherwise it fails
  atomically and leaves the request queued. The queue does *not* reserve idle
  liquidity against later atomic exits.

**Governance control-plane boundary.** Vault-bound governance actions cross from
the governance contract into the runtime through a single bridge,
`execute_governance(env, caller, payload)`. Emergency **pause and restriction
tightening are immediate Sentinel actions**; **unpause and relaxing/removing
restrictions are governance actions that must clear the configured timelock**
before the runtime applies them.

**Fee anchor & idle reconciliation.** Unsolicited underlying transfers are
treated as idle assets for *existing* shareholders, not as profit the next
depositor can capture. Deposits, fee refreshes, and idle resyncs read the live
asset balance, update `idle_assets`, and reset the `fee_anchor`. When fees are
active, deposits **crystallize elapsed fees first**, so deposit principal cannot
erase already-accrued fees.

**TTL keeper responsibility.** Soroban storage is not permanent. A vault
deployment **must** include a keeper that periodically calls the permissionless
`ExtendTtl` path. Related contracts (share token, governance, adapters, proxy,
oracle) each need their own TTL maintenance — they do not inherit the vault
runtime's renewal.

## Curator economics (fees)

There are two fee types. Both are **minted as new SEP-41 shares** to a
configurable recipient (so a recipient must be able to hold shares to receive
them).

| Fee | Basis | Cap |
|-----|-------|-----|
| **Management** | Time-weighted on AUM (`rate × AUM × elapsed / 1yr`), accrues regardless of performance | **5% / year** |
| **Performance** | AUM **growth** since the last accrual checkpoint; zero on flat or down periods | **50% of profit** |

Rates are WAD-scaled (`1e18 = 100%`). Each fee has its own recipient, and they
can differ. Two nuances worth understanding:

- **Checkpoint, not all-time high-water mark.** Fees accrue on *every*
  interaction, and the fee anchor resets to current AUM each time. Profit is
  `current_AUM − anchor_AUM`. If AUM is flat or down versus the anchor, the
  performance fee is zero — but because the anchor resets downward after a loss,
  a subsequent recovery *is* chargeable. This is "growth since the last
  checkpoint", not "growth above the all-time peak".
- **Anti-donation cap** (`max_total_assets_growth_rate`, optional). Caps how fast
  AUM is allowed to count toward fees, preventing a one-block donation from
  inflating fees. Relaxing or removing this cap is timelocked.

## Governance switches and the timelock rule

**The principle behind every switch:** changes that **disadvantage depositors**
are **timelocked** (so depositors can exit first, and the Sentinel can veto);
changes that **protect or benefit depositors** take effect **immediately**.
Timelocks are configurable per kind, bounded between **0 and 30 days** (default 2
days).

| Switch | Immediate (depositor-friendly) | Timelocked (depositor-adverse) |
|--------|-------------------------------|-------------------------------|
| **Fees** | Fee **decrease** | Fee **increase**, recipient change, relaxing the growth cap |
| **Market cap** | **Lower** cap (incl. set to 0 = stop deposits) | **Raise** cap, or a **new** market |
| **Cap group** (absolute + relative) | **Tighten** (lower / add a cap) | **Loosen** (raise / remove a cap). Relative cap ≤ 100% |
| **Restrictions** | **Pause / tighten** (blacklist, narrow whitelist) | **Unpause / relax** |
| **Timelock length** | **Lengthen** | **Shorten** (waits under the old, longer timelock) |
| **Market removal** | — | Always timelocked; requires the cap to already be 0 |

Other levers:

- **Supply queue** — the ordered list of allocation targets (up to 64 markets, no
  duplicates, every market must have a cap > 0).
- **Cap groups** — cluster correlated markets under a shared limit; the effective
  limit is `min(absolute_cap, relative_cap × total_AUM)`.
- **Cooldowns** — withdrawal (default 1 hour), market refresh, idle resync.
- **Abdicate** — permanently and irreversibly disable a governance method.

## Worked examples

1. **Raising the performance fee 10% → 20%** is timelocked; depositors can
   withdraw and the Sentinel can revoke. *Lowering* it applies instantly.
2. **A market turns risky.** Cutting its cap 1M → 500k is **immediate**; setting
   it to **0** stops new allocations now. *Raising* a cap is timelocked.
3. **Incident response.** Pausing is **immediate**; *un-pausing* is timelocked,
   so users get notice before normal operation resumes.
4. **A cap group "blue-chip"** with absolute 5M and relative 40%: at 8M AUM the
   cluster holds at most `min(5M, 0.40 × 8M) = 3.2M`; it scales with AUM until
   the 5M ceiling binds.
5. **Fee accrual.** The vault grows 10M → 11M; a 20% performance fee mints ~200k
   (assets-equivalent) of shares to the performance recipient (20% of the 1M
   gain), plus time-weighted management shares on the 10M base. Both dilute
   existing holders by exactly the minted amount.

## Operating a vault: the vault CLI / client SDK

Day-to-day curator and allocator operations run through the **vault client SDK**
("the vault CLI"). It locks in a focused set of production-ready flows — with
proper fee/gas attachment, nonce handling, and retry logic — rather than
exposing the full contract surface.

**Full reference:**
<https://github.com/Templar-Protocol/contracts/blob/dev/client/vault/README.md>

Highlights:

- **Bindings for multiple languages** via UniFFI — Python, TypeScript, and Rust —
  generated from the vault contract ABI, so the same flows are available to
  automation and to the frontend.
- **Curator / allocator flows**: `deposit`, `refresh_markets` (update the vault's
  view of market assets), `reallocate` (supply to / withdraw from a market),
  `withdraw` / `redeem`, `execute_withdrawal`, and config calls such as
  `set_fees`.
- **View & preview helpers**: `get_total_assets`, `get_idle_balance`,
  `get_configuration`, `get_fees`, `preview_deposit` / `preview_withdraw` /
  `preview_redeem`, and `build_real_assets_report`.
- **Production reliability**: multi-key pooling with least-loaded selection,
  per-key nonce management with retry on stale nonces, TTL-based view caching,
  and zeroizing key handling. Built for unattended curator/allocator bots.

Typical reallocation (curator operation):

```python
from templar_vault_client import AllocationDelta, Delta

# Supply idle assets to a market
await client.reallocate(AllocationDelta.Supply(Delta(market=market_id, amount=amount)))

# Pull assets back from a market
await client.reallocate(AllocationDelta.Withdraw(Delta(market=market_id, amount=amount)))
```

## Curated-vault frontend (UI)

For curators who prefer a UI over scripting, the **curated-vault frontend at
<https://app.templarfi.org/vaults/curator/>** is the reference interface over the
same vault operations the CLI/SDK exposes (deposit, withdraw/redeem, refresh,
reallocate, fee and policy changes). It builds the transactions with the correct
gas/deposit policy and **delegates signing to the user's connected wallet** — the
frontend never handles private keys. It is the no-code path to the same flows
documented above.
