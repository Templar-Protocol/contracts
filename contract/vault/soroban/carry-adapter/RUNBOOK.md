# Carry Adapter Runbook

Operational protocol for `templar-soroban-carry-adapter`. The adapter borrows the yield-pool
asset against vault collateral on the collateral `pool`, deploys the borrowed leg into
`yield_pool`, and reports NAV `C + conv(Y − D)` through the pool oracle.

## Deploy gate (step 0, economic)

Deploy only when the carry spread is positive on default fork curves:

- The yield-pool borrow side must run at **>= ~85% utilization**, and
- The collateral pool must sit near its target, around **5% APR**.

Below ~85% borrow utilization the yield leg does not earn enough to cover the collateral-side
borrow cost; on default `blend-contracts-v2` fork curves (pinned at
`54afdae01bb8e484514940229e06460aa3fa6a1f`, hashes in
`contract/vault/deployments/carry-adapter/manifest.template.json`) the carry is net-negative
underneath that threshold. Verify utilization and APR against the live fork curve before
filling the manifest, and rerun this check at every `set_params`.

## Curator protocol (daily operations)

For each allocation cycle:

1. `refresh_markets` (include the carry market) — validates NAV against caps.
2. `allocate_supply` in an **adjacent transaction**.

The kernel supply window is `−amount + ε <= drift <= ε`; if the refresh is not adjacent, the
allocation fails with `supply observation out of bounds` — simply retry the refresh/allocate
pair. Withdraw allocations cap `amount` at the adapter's stored principal; the adapter never
releases more than that.

Oracle failure outside band reverts `total_assets`: leave this market out of the explicit
`refresh_markets` list until the feed recovers.

## Surplus realisation (sweep flow)

When `Y > D` (surplus leg):

1. Admin calls `sweep_surplus()` — withdraws from `yield_pool` (respecting the liquidity cap),
   keeps `D * (1 + surplus_buffer_bps/1e4)` reserved, pays the surplus borrow leg to the
   immutable `surplus_recipient` converter, and **sets `realisation_pending`**.
2. The converter swaps borrow-asset → collateral-asset off-contract and **donates the
   collateral to the adapter address**.
3. Admin calls `clear_realisation_pending()`.
4. The curator immediately runs `refresh_markets`.

While `realisation_pending` is set:

- `supply()` and `total_assets()` revert — `allocate_supply` fails and the carry market MUST be
  excluded from the `refresh_markets` list. A refresh that names the market while pending fails;
  remove it from the list until the flow completes.
- `progress_withdrawal()` and vault withdrawals keep working at the stored principal, which
  hides real moves of `Y − D`. **Bound the window to hours, not days**; monitor
  `realisation_pending()` age and page if it exceeds the window.

Donated collateral counts as ordinary NAV growth through the normal refresh path (performance
fees apply). Third-party donations to the adapter in the window are NAV growth and only delay
the next `allocate_supply` (retry resolves it).

## Shortfall funding

When `D > Y` (shortfall): NAV deducts the shortfall; the **named funder** (curator/treasury
account recorded in the manifest) tops the borrow leg back up by transferring borrow-asset to
the adapter. There is no write-off path: the vault's `remove_market` (both the policy-admin and
governance routes) rejects any market with non-zero stored principal, so principal reaches 0
only through NAV reaching 0 (full exit after funder top-up, or organic decay). The stranded
tail stays in NAV until then; funder top-up is the only resolution.

## Wind-down order

Never skip the realisation step: withdrawing a principal larger than collateral `C` releases
only `C`.

1. If `Y > D`, realise surplus first (sweep → converter → donate → clear → refresh) so the
   stored principal equals NAV.
2. Withdraw the full principal (idle-first delivery; pool-free if the pool part is 0).
3. If `Y > 0` remains at `D == 0` (the buffer plus accrual), sweep again → converter → donate →
   clear → refresh, then do an idle-only withdraw of the donated collateral.
4. `refresh_markets` to book principal = 0 (NAV > 0 would block the next steps).
5. Governance: set the market cap to 0 (a NAV > 0 market with cap 0 fails refresh).
6. `remove_market` (requires stored principal == 0 on both routes).

## Pool-block recovery

Pool-side conditions can block supply or unwind. Recovery per condition:

| Condition | Supply | Unwind | Recovery |
|---|---|---|---|
| Health factor below threshold | blocked | reverted | `deleverage` (admin, includes delete-auction when applicable), then unwind |
| Auction in progress | blocked | reverted | wait for auction, or `deleverage` with delete-auction |
| Pool status 2/3 | blocked | OK | wait for status < 2; unwinds pass |
| Pool status >= 4 | reverted | OK | unwind passes; investigate pool incident |
| 100% collateral-pool utilization | capped | partial | partial unwind via the utilization cap; `deleverage` if needed |
| `min_residual_collateral` floor | capped | capped | cap keeps `min_residual_collateral` on the pool; residual stays until pool liquidity returns |

`pause` (`set_paused`) blocks `supply` only; withdrawals and deleverage remain available — use
it to halt new carry without impeding exits.

## Monitoring

- `position()` — poll collateral `C`, debt `D`, yield leg `Y`, and NAV every cycle.
- `realisation_pending()` — alert on any window longer than the bounded realisation window
  (hours).
- Negative-carry monitor: compute realized yield-leg APR minus borrow cost APR from `position()`
  deltas between refreshes; alert when the spread turns negative or borrow utilization falls
  below the ~85% deploy-gate threshold, then wind down per the order above if it persists.

## Development

```
cargo test -p templar-soroban-carry-adapter --features testutils
```

Runs the fork-pool harness tests (harness_smoke, adapter_integration, auth_tree, adversarial);
the harness and mock oracle live behind the `testutils` feature.
