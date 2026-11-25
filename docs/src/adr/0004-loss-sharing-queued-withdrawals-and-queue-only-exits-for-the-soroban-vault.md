# 4. Loss-sharing queued withdrawals and queue-only exits for the Soroban vault

Date: 2026-11-25

## Status

Proposed

UNREVIEWED: drafted without an Architect or Critic review, at the protocol owner's request. It
needs that review before a human considers accepting it.

## Context

A holder can leave the Soroban vault in two ways. Both let the holder step out ahead of a loss that
the vault has not booked yet, and leave that loss to the holders who stay.

- A queued withdrawal is a fixed claim. `request_withdraw` prices the escrowed shares once, and
  execution pays that amount when idle assets cover it. The README calls this an intentional
  tradeoff: a later fall in value is absorbed by the remaining share supply, not by the request.
- An immediate exit is paid from idle assets at once, at the value the vault last booked. It checks
  pause, restrictions, slippage and the idle balance. It does not check how old the booked value
  of the vault's markets is; only a deposit does.

The vault books a market's value when the allocator allocates or refreshes, so a loss in a market
reaches the share price only then.

For a vault of ordinary supply positions the README leaves this to cooldowns and allocator
process. That is not enough for a vault that holds a first-loss position: one that takes a
market's losses before other suppliers do, and that the market releases only after a notice
period. Such a position is worth something to the market's suppliers because it stays in place
until a loss has landed. A vault that holds it for many funders has to pass that wait on to them.
Today it cannot. A funder who sees a loan go bad can leave through idle assets at once, or queue a
fixed claim, and the funders who stay take the whole loss.

No existing setting separates the two exit paths. A pause blocks the queue as well, and so do
restrictions for the accounts they list.

Constraints:

- The runtime's release artifact is 129,507 bytes against a limit of 131,072, which leaves 1,565.
- A queue entry is stored as 112 bytes inside the state blob, and an upgrade refuses to migrate a
  vault with virtual assets while withdrawals are pending. Stored formats therefore do not change.
- `min_assets_out` is checked when a withdrawal is requested and is not stored.
- Existing vaults keep today's behaviour unless their governance opts in.

Evidence at snapshot `7068e576cda13fd0285ee8a318ec3854e1e10074`, paths under `contract/vault`:

- `kernel/src/actions/mod.rs:1121-1133,2196-2208` prices a request with
  `convert_to_redeem_assets_bounded`, rounded down, and checks `min_assets_out` there.
  `kernel/src/state/queue/mod.rs:34-40` is the queue entry: owner, receiver, `escrow_shares`,
  `expected_assets` and the request time.
- `kernel/src/actions/mod.rs:2040-2060` and `kernel/src/state/queue/mod.rs:72-104` pay
  `expected_assets` and burn the whole escrow once idle assets cover the claim.
  `soroban/README.md:156-161` states the tradeoff.
- `kernel/src/actions/mod.rs:1257-1298` is the immediate withdrawal.
  `soroban/proxy-4626/src/contract.rs:209,239` exposes it as `atomic_withdraw` and
  `atomic_redeem`; that proxy's `withdraw` and `redeem` are queued requests.
- `soroban/src/contract/entrypoints.rs:613-628` refuses a deposit while the booked value of the
  vault's markets is older than `max_nav_age`. No exit path has that check.
- `curator-primitives/src/rbac/mod.rs:267-269` and `kernel/src/actions/mod.rs:968-974` show that a
  pause and restrictions apply to both exit paths.
- `soroban/src/contract/types.rs:192-195` and `soroban/shared-types/src/lib.rs:697-706` hold the
  per-vault settings as instance-storage values behind numbered configuration kinds.
  `soroban/governance/src/lib.rs:254-284,857-863` submits them under `TimelockKind::Fees`.
- `soroban/src/storage/mod.rs:799-825` serializes the queue entry. `soroban/README.md:180-183`
  states the migration rule. `soroban/AGENTS.md:77-78` records the artifact size.
- `kernel/src/effects.rs:174-180` is `PayoutCompleted`, which already carries the amount paid.

| Scope | Disposition |
|---|---|
| Cap a queued payout at what its escrowed shares are worth when it is paid, per vault | Include |
| Turn off immediate exits, per vault | Include |
| Refuse a capped payout that would be priced on an old booked value | Include; it reuses the deposit check |
| An adapter contract that holds a first-loss position | Defer; a new contract needs its own decision |
| Fees that accrue on a queued claim after the request (ENG-697) and a per-share high-water mark (ENG-701) | Defer; tracked separately |
| Paying a queued claim more than its request-time value after the vault gained | Reject; see Alternatives |
| Storing `min_assets_out` in the queue entry | Reject; it changes a stored format |

## Decision

1. Add one per-vault setting, `exit_policy`, holding two flags: `CAP_QUEUED_PAYOUT` (1) and
   `NO_IMMEDIATE_EXIT` (2). It is an instance-storage value behind a new configuration kind, like
   `maxnav` and `wdcooldn`. An absent value is zero, and zero is today's behaviour. A value above 3
   is rejected. The governance contract gains `submit_set_exit_policy`, mapped to
   `TimelockKind::Fees` as the withdrawal cooldown is, so every change waits that timelock in both
   directions.
2. With `CAP_QUEUED_PAYOUT`, a queued withdrawal is paid the lower of its stored `expected_assets`
   and what its escrowed shares redeem for at payment. Execution first runs the fee refresh that
   `request_withdraw` runs before it prices, then values the escrowed shares with
   `convert_to_redeem_assets_bounded`. The whole escrow is burned in both cases. The payout still
   runs only when idle assets cover the amount to be paid. The stored `expected_assets` is not
   rewritten.
3. With `CAP_QUEUED_PAYOUT`, `execute_withdraw` fails while the vault has external assets whose
   booked value is older than `max_nav_age`. This is the check a deposit already makes, and it keeps
   its meaning: a `max_nav_age` of `u64::MAX` turns it off.
4. With `NO_IMMEDIATE_EXIT`, `AtomicWithdraw` and `AtomicRedeem` fail before any state change, with
   an error that a caller can tell from a pause. `RequestWithdraw` and `ExecuteWithdraw` are not
   affected. The proxy's `atomic_withdraw` and `atomic_redeem` fail with it, and a proxy view that
   quotes an immediate exit reports zero.
5. The policy in force when a call runs applies. A request queued before the cap was turned on is
   capped if it is paid after. The timelock is the notice.
6. A request whose capped amount is zero is handled by the existing rule for a zero-value request
   (`soroban/README.md:185-187`). It never blocks the head of the queue.
7. No stored format changes: `VaultState`, the state blob and the queue entry keep their bytes, and
   an upgrade needs no migration. No event changes: `PayoutCompleted.amount` is the amount paid.
   `min_assets_out` stays a request-time check, so a capped payout can be below it.
8. The release artifact stays at or below 131,072 bytes with both flags. The implementing change
   reports the artifact's size before and after. If the flags do not fit after the low-delta
   slimming that `soroban/AGENTS.md` prescribes, they do not ship under this record, and a successor
   says what gives way.

## Alternatives

- Reprice a queued claim fully at payment, up as well as down. It is simpler to state, but it hands
  a leaver gains and unsettled fees from after the request. ENG-697 owns that question.
- Lengthen the cooldown only. Payment comes later, but the claim is still fixed at the request, so
  a loss in between still falls on the holders who stay.
- Apply the `max_nav_age` check to immediate exits and keep them. That stops an exit on an old
  value. It does not stop an exit ahead of a loss the market itself has not booked, such as a loan
  that is under water and not yet liquidated.
- Use a pause or restrictions. Both block the queue as well as the immediate path.
- One flag for both behaviours. It saves a few bytes, but a vault of ordinary positions may want
  the cap and still offer immediate exits. Two flags in one setting cost one configuration kind.
- Store `min_assets_out` and refund the shares when the capped amount is below it. It changes the
  112-byte queue entry and needs a migration, and a holder of first-loss shares cannot be promised a
  floor against losses.
- Enforce queue-only exits in a wrapper around the vault. The runtime accepts the immediate
  commands directly through `execute`, so a wrapper cannot close that path.
- Leave the vault as it is and disclose. That holds for a vault its curator funds alone, where the
  one holder cannot leave ahead of itself. It does not hold once outside funders share the vault.

## Consequences

With the cap, a holder who queues shares every loss the vault books until the payment, and no gain.
The request-time slippage bound no longer holds at payment. With immediate exits off, every exit
waits for the cooldown and for idle assets.

The cap acts on the booked value, so it depends on the allocator refreshing. A loss the market has
not booked is in no value at all, and a holder paid before it is booked still avoids it. An operator
narrows that by setting the withdrawal cooldown at least as long as the notice period of the
position the vault holds. That, and whether such a vault charges a performance fee before ENG-701,
are operator settings and not part of this decision.

Integrators that rely on immediate exits lose them in a vault that sets the flag. The setting is
opt-in and timelocked, and the proxy's views have to say so.

Acceptance requires:

- Kernel and runtime tests with the policy absent: every existing queued-withdrawal test passes
  unchanged, among them `settle_payout_success_burn_only` in the kernel and
  `test_execute_withdraw_insufficient_idle_refuses_partial_settlement` in the runtime.
- Kernel tests with the cap: a fall in value after the request pays the current value and burns the
  whole escrow; a rise pays `expected_assets`; a payout never lowers the assets per share of the
  remaining supply; a zero capped amount does not block the queue.
- Runtime tests: `NO_IMMEDIATE_EXIT` rejects both immediate commands and leaves request and
  execution working; a capped payout is refused on an old booked value and passes after a refresh;
  a value above 3 is rejected; the setting changes only through the governance timelock.
- A stored-state test: a state blob with pending withdrawals written by the released runtime loads
  and settles under the new code, with the policy absent, to the same result.
- Proxy tests for the two atomic methods and for the views that quote an immediate exit.
- The size gate, with the artifact's bytes before and after, and the release ABI check.
- The README's sections on queued and immediate exits, updated in the same change.

Run the vault guide's minimum runtime verification through the repository's `just` entrypoints,
then `just -f contract/vault/soroban/justfile size-budget-check` and `verify-release-wasm-abi`.
Deployment and turning the policy on for any vault are not authorized by this decision.

| Failure scenario | Warning and prevention | Detection and recovery |
|---|---|---|
| The allocator stops refreshing, so capped payouts are refused | Watch the age of the last refresh against `max_nav_age` | Any allocator refreshes. Governance can raise `max_nav_age` through the timelock |
| Governance turns the policy off so that insiders can leave ahead of a loss | The timelock gives notice, and the cap stays in force until the change lands | The governance proposal is on chain. Holders can queue during the timelock |
| A capped payout below the request-time bound surprises an integrator | Document that the bound holds at the request only | `PayoutCompleted.amount` against the queue entry's `expected_assets` |
| The market has not booked a loss when a claim is paid | Set the cooldown at least as long as the position's notice period | None on chain. The holders who stay take that loss |
| The two flags do not fit the size limit | Measure the artifact before the tests are written | A successor record decides what gives way |

The adapter for a first-loss position, and the fee questions in ENG-697 and ENG-701, belong in
their own reviews.
