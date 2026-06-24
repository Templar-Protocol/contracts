# Vault Curator Guide (Soroban)

This guide explains how a Templar **Soroban** vault is structured and the economic
and governance levers available to a curator: the fees a curator earns, the
configurable risk "switches" and their timelock rules, and the tooling
(CLI/SDK and frontend) used to operate a vault day to day.

> **Key references**
> - **Architecture & code:** [`contract/vault/README.md`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/README.md) (kernel + executor design, state machine, withdrawal/allocation flows). Soroban runtime specifics live in [`contract/vault/soroban/README.md`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/soroban/README.md).
> - **Vault CLI / client SDK:** [`client/vault/README.md`](https://github.com/Templar-Protocol/contracts/blob/dev/client/vault/README.md).
> - **Curated vault frontend (UI):** [app.templarfi.org/vaults/curator](https://app.templarfi.org/vaults/curator/).

## High-level vault structure

A Templar vault is a **single-asset, ERC-4626-style yield vault**. Depositors
supply one underlying SEP-41 token and receive transferable **SEP-41 shares**;
the curator allocates the pooled assets across a chosen set of on-chain lending
**markets** (adapters) to earn yield.

**Kernel + executor architecture** (see the [architecture README](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/README.md)):

- `templar-vault-kernel` — chain-agnostic source of truth: state machine, math,
  fee accrual, and invariants. It returns *effects* (mint/burn/transfer/emit)
  rather than touching chain state directly.
- **Soroban executor** (`contract/vault/soroban`) — `SorobanVaultContract`
  entrypoints wrap `CuratorVault<S, A, E>`, which loads versioned state, enforces
  RBAC via `require_auth()` + `ActionKind`, applies the kernel action, and runs
  the resulting effects against the SEP-41 share and asset tokens.
- **Governance contract** (`contract/vault/soroban/governance`) — a *separate*
  contract that owns proposal submission, timelocks, approvals, revocation, and
  abdication. Vault-bound changes cross the boundary through a single bridge,
  `execute_governance(env, caller, payload)`; the runtime remains the canonical
  owner of applied config/policy state.
- `contract/vault/curator-primitives` — shared policy/RBAC/governance helpers
  (caps, cap groups, supply queue, timelocks, restrictions).

**Accounting invariant:** `total_assets = idle_assets + external_assets`.

- `idle_assets` — uninvested buffer held by the vault; also the liquidity source
  for immediate withdrawals (there is no separate "idle market"). Unsolicited
  direct transfers into the vault are reconciled as idle assets for existing
  shareholders, not captured as profit by the next depositor.
- `external_assets` — principal deployed into markets via adapters.

**Two withdrawal modes** (a Soroban-specific design point):

- `withdraw` / `redeem` — **atomic** ERC-4626-style exits from **idle liquidity
  only**. They never enqueue work or pull from adapters, and fail if the request
  exceeds `idle_assets`. (The 4626 proxy's `maxWithdraw`/`maxRedeem` are bounded
  by idle assets and can read `0` even when a holder's shares are backed by
  market-deployed assets.)
- `request_withdraw` → `execute_withdraw` — the **async, keeper-routed** path for
  positions that need allocator work. `request_withdraw` escrows shares, starts a
  cooldown, and locks a fixed `expected_assets` claim; `execute_withdraw`
  (allocator-authorized) settles the head request only when it is cooled down and
  fully covered by idle assets, otherwise it fails atomically and leaves the
  request queued.

**Roles:**

- **Owner** — top-level governance.
- **Curator** — policy admin: market caps, cap groups, supply queue, market
  removal. Implicitly holds the Allocator role.
- **Allocator** — operational keeper: allocations, rebalances, `execute_withdraw`,
  refreshes.
- **Sentinel** — emergency authority (a *separate* role holder; the governance
  contract is **not** implicitly the Sentinel): pause / tighten restrictions
  immediately, abort in-flight operations, and revoke pending timelocked changes.

**Operational note — TTL keeper.** Soroban contract data is not permanent. Every
vault deployment must run an ops job that periodically calls the permissionless
`ExtendTtl` path. Related contracts (share token, governance, adapters, 4626
proxy, oracle) each maintain their **own** TTL and do not inherit the vault's
renewal.

## Curator economics (fees)

There are two fee types. Both are **minted as new SEP-41 shares** to a
configurable recipient.

| Fee | Basis | Cap |
|-----|-------|-----|
| **Management** | Time-weighted on AUM (`rate × AUM × elapsed / 1yr`), accrues regardless of performance | **5% / year** |
| **Performance** | AUM **growth** since the last accrual checkpoint; zero on flat or down periods | **50% of profit** |

Rates are WAD-scaled (`1e18 = 100%`). Each fee has its own recipient, and they
can differ.

Two nuances worth understanding:

- **Checkpoint, not all-time high-water mark.** On Soroban, share-pricing paths
  (`DepositWithMin`, `RefreshFees`, `ResyncIdleBalance`) first reconcile
  `idle_assets` against the live asset-token balance, then reset the `fee_anchor`
  to the reconciled total at the current ledger time. Profit is measured as
  `current_AUM − anchor_AUM`; if AUM is flat or down, the performance fee is zero.
  Because the anchor resets after each interaction, a recovery following a loss is
  chargeable — this is "growth since the last checkpoint", not "above the all-time
  peak". When fees are active, a deposit first crystallizes elapsed fees before
  the post-deposit anchor is written, so deposit principal cannot erase accrued
  fees.
- **Anti-donation cap** (`max_total_assets_growth_rate`, optional). Caps how fast
  AUM is allowed to count for fee accrual:
  `effective_AUM = min(current, last × (1 + max_rate × dt/yr))`. Relaxing or
  removing this cap is timelocked.

## Governance switches and the timelock rule

**The principle behind every switch:** changes that **disadvantage depositors**
are **timelocked** (so depositors can exit first, and the Sentinel can veto);
changes that **protect or benefit depositors** take effect **immediately**.
Timelocks are configurable per kind, bounded between **0 and 30 days** (default 2
days).

On Soroban, immediate Sentinel actions (pause, tighten restrictions) are applied
directly; everything timelocked is submitted to the governance contract and only
applied by the runtime after the delay via `execute_governance`.

| Switch | Immediate (depositor-friendly) | Timelocked (depositor-adverse) |
|--------|-------------------------------|-------------------------------|
| **Fees** | Fee **decrease** | Fee **increase**, any **recipient change**, relaxing the growth cap |
| **Market cap** | **Lower** cap (incl. set to 0 = stop deposits) | **Raise** cap, or a **new** market |
| **Cap group** (absolute + relative) | **Tighten** (lower / add a cap) | **Loosen** (raise / remove a cap). Relative cap ≤ 100% |
| **Restrictions** | **Pause / tighten** (blacklist, narrow whitelist) | **Unpause / relax** |
| **Skim recipient** | — | Recipient change and skim execution are timelocked |
| **Timelock length** | **Lengthen** | **Shorten** (waits under the old, longer timelock) |
| **Market removal** | — | Always timelocked; requires the cap to already be 0 |

Other levers:

- **Supply queue** (`set_supply_queue`) — the ordered list of allocation targets.
  Up to 64 markets, no duplicates, every market must have a cap > 0, and the
  vault must be Idle.
- **Cap groups** — cluster correlated markets under a shared limit. The effective
  limit is `min(absolute_cap, relative_cap × total_AUM)`.
- **Allocator and adapter-allowlist** changes are routed through
  `execute_governance`.
- **Cooldowns** — withdrawal (default **1 hour**), market refresh (30 seconds),
  idle resync (120 seconds).
- **Abdicate** — permanently and irreversibly disable a governance method (for
  example, to lock fees forever).

**Adapters (markets).** Soroban ships two adapter types you allow-list and add to
the supply queue before allocation:

- **Blend adapter** (`contract/vault/soroban/blend-adapter`) — integrates a Blend
  lending pool.
- **Custodial adapter** (`contract/vault/soroban/custodial-adapter`) — an
  offchain-managed route that forwards assets to a configured custodian/multisig;
  its NAV is explicit reported accounting. Treat the custodian and its offchain
  process as part of the vault's trust boundary, and verify the custodial runbook
  before production use.

## Worked examples

1. **Raising the performance fee 10% → 20%** is timelocked (e.g. 2 days):
   submitted to the governance contract, applied by the runtime only after the
   delay; depositors can exit and the Sentinel can revoke. *Lowering* it 20% → 10%
   applies instantly.
2. **A market turns risky.** Cutting its cap 1M → 500k is **immediate**; setting
   it to **0** stops new allocations now (the first step of winding it down).
   *Raising* a cap 1M → 2M is timelocked.
3. **Incident response.** Pausing is an **immediate** Sentinel action; *un-pausing*
   is a governance action that must pass the timelock, so users get notice before
   normal operation resumes.
4. **A cap group "blue-chip"** with an absolute cap of 5M and a relative cap of
   40%: at 8M AUM the cluster can hold at most `min(5M, 0.40 × 8M = 3.2M) = 3.2M`.
   The limit scales with AUM until the 5M ceiling binds.
5. **Fee accrual.** The vault grows 10M → 11M between interactions. With a 20%
   performance fee, roughly 200k (assets-equivalent) of shares mint to the
   performance recipient (20% of the 1M gain); a 2%/yr management fee additionally
   mints time-weighted shares on the 10M base for the elapsed interval. Both are
   dilutive to existing holders by exactly the minted share amount.

## Operating a vault: CLI and frontend

### Vault CLI / client SDK

The vault client SDK ([`client/vault/README.md`](https://github.com/Templar-Protocol/contracts/blob/dev/client/vault/README.md))
is purpose-built for **curator/allocator automation**. Rather than exposing the
full contract surface, it locks in a focused set of curated, production-ready
flows with proper fee/deposit attachment, nonce handling, and retry logic:

- **Curator/allocator operations:** `reallocate` (supply/withdraw a market),
  `refresh_markets`, `set_fees`, `execute_withdrawal`.
- **User-style flows:** `deposit`, `withdraw` / `redeem` (two-phase:
  preview → request → execute), `storage_deposit`.
- **Views & previews:** `get_total_assets`, `get_idle_balance`, `get_fees`,
  `get_configuration`, `get_restrictions`, `convert_to_shares` / `convert_to_assets`,
  `preview_deposit` / `preview_withdraw` / `preview_redeem`,
  `build_real_assets_report` — 50+ methods generated via the
  `impl_vault_methods!` macro.

It ships **type-safe bindings** for **Python** (native async), **TypeScript**
(generated from the contract ABI), and **Rust** (direct library usage) via
UniFFI, plus production conveniences: a multi-key pool with least-loaded
selection, per-key nonce caching with retry, TTL-based view caching, zeroizing
key handling, and health/observability reporting. See the README for prepared
deposit/withdraw/redeem/refresh/reallocate flow examples and client configuration.

### Curated vault frontend

The [curated vault UI at **app.templarfi.org/vaults/curator**](https://app.templarfi.org/vaults/curator/)
is the reference frontend for curators — a wallet-based interface over the same
vault operations the CLI/SDK exposes (deposit, withdraw/redeem, refresh,
reallocate, and curator governance actions). It builds transactions with the
correct parameters and delegates all signing to the user's connected wallet; the
frontend never handles private keys. Use it to drive vault operations interactively
without scripting against the SDK.

### Soroban on-chain operations

For deployment and direct on-chain interaction, the Soroban runtime ships
`justfile` recipes (`setup`, `deploy-all`, `demo-deposit`, `demo-withdraw`,
adapter build/deploy) driven by `stellar-cli` v26. See
[`contract/vault/soroban/README.md`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/soroban/README.md)
for prerequisites, the deployment artifact/size budget, state-size limits, and the
TTL keeper requirement.
