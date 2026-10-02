# templar-vault-near

NEP-461 wrapped-asset vault with epoch settlement and Allocator-routed markets.

This is the NEAR runtime for the vault boundary. It keeps the NEAR-facing state and
entrypoints while settlement, withdrawal and deposit-admission law is enforced
exclusively by `templar-vault-kernel`, reached through the internal mirror in
`src/kernel_mirror.rs`.

## Authority and settlement law

- **Settlement authority is idle-only.** `settle_epoch` runs only while the vault is
  idle, and only over what this contract measured itself. The settlement NAV is the
  vault's own idle balance and the eligible supply is its own share total.
- **A local settlement snapshot is trusted only when the vault holds nothing outside
  idle.** Concretely: settlement is refused while any market holds principal, and it
  is refused unless the ledgers account for every unit this contract holds, including
  assets recorded as held-but-unadmitted deposits.
- **Consequence, by design: market exposure halts settlement permanently rather than
  repricing it.** NEAR has no authenticated market-adapter valuation that settlement
  could consume, so assets deployed outside idle cannot be priced. When that happens
  the vault fails closed: settlement stops and withdrawals stay halted until the
  assets are recalled and back in idle. A member is never paid against a stale or
  self-reported price. Recall the market principal, confirm the idle balance, and
  settlement resumes at the next cutoff.
- Each epoch settles once against one snapshot, and only after its cutoff. Accepted
  snapshots carry no asset balance: only the idle balance and share supply are bound,
  so the snapshot prices obligations without re-pricing custody.
- Claims are per-epoch and derived from the snapshot for the epoch a request was
  queued in. Nothing stores or accepts a fixed asset claim: a request records only
  identity, escrowed shares, an optional minimum-asset floor, request time, and its
  intake epoch.

## Exit requests and floors

- The standard exit ABI is unchanged and carries no floor: `withdraw` and `redeem`
  record `min_assets_out = 0`, because that ABI has no field for one.
- A member who refuses a settlement price must say so through `withdraw_with_min` or
  `redeem_with_min`. The floor is stored with the request and enforced at settlement
  and payout: a claim below it halts that member's exit instead of paying a price they
  refused. It is a refusal, never a quote.
- A payout pays only the head of the queue, only at or above that request's stored
  floor, and only at the amount re-derived from the accepted settlement at execution
  time. After a successful transfer the request's **entire** escrow is burned and the
  queue head is released, so escrow cannot outlive the obligation it secured.

## Protected deposit intake and admission

- Deposits that arrive while an epoch is closed for settlement, or while delayed
  valuation protection applies, do **not** mint shares and do **not** enter total
  assets or the idle balance. They are recorded as liabilities already in the vault's
  custody, with owner, assets, the caller's `min_shares_out` floor, request time, and
  intake epoch. If recording fails, the assets are returned in full before any
  accounting changes.
- Two and only two paths leave a recorded liability, and neither is automatic: the
  **owner** may call `refund_pending_deposit`, which releases the assets and removes
  the liability only after the transfer is confirmed successful by callback, or the
  **Allocator** may call `admit_pending_deposit`.
- **Admission authority is `Allocator`**, enforced through the ordinary
  role-to-action policy. Admission is refused unless the vault is idle, holds nothing
  outside idle, and an accepted settlement snapshot covers the deposit's own intake
  epoch. Shares are then derived solely from that snapshot's NAV and eligible supply,
  the depositor's stored `min_shares_out` is enforced against that figure, and no
  second asset transfer occurs, because the assets never left the vault. The
  liability and its floor are removed only after the mint is applied and the assets
  are booked once into the idle balance. A replayed admission finds no liability.
- Held-but-unadmitted assets are excluded from NAV, the idle balance, share supply,
  and every fee or allocation basis, and they cannot be deployed to a market or
  skimmed. `pending_deposit_total` reports what is held pending release or admission;
  settlement refuses to run while that figure and the liability records do not
  reconcile.

## Member visibility

`get_settled_claim(account, request_id)` reports the settled claim an accepted
snapshot derives for a request, with nothing owed before settlement covers that
request's epoch. `get_epoch_settlement_status(request_id)` reports whether the epoch a
request was queued in has settled; that is when the claim becomes knowable, because a
delayed-valuation settlement price is not knowable at request time.
`get_pending_deposits(account)` lists every recorded-but-unadmitted deposit held for
an account, oldest first, and `pending_deposit_total()` reports the outstanding held
total.

## Recovering a held deposit

If you deposited while an epoch was closed or valuation was protected, your assets
were held instead of priced. Call `get_pending_deposits` with your own account id to
see each request you have: its identifier, the assets held, when you made the request
and the epoch it belongs to.

You can always call `refund_pending_deposit` with one of those identifiers to have the
held assets returned to you; the holding is released only once that transfer has
actually succeeded. Alternatively, once the settlement covering that epoch has been
accepted, the vault's Allocator can call `admit_pending_deposit` for the request,
which credits you the shares that settlement prices for your assets and never charges
you a second time. Both routes end the holding; neither can price your deposit at the
moment you sent it, because a delayed-valuation settlement price does not exist yet at
that moment.
