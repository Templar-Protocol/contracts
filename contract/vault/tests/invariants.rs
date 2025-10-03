// TODO: single-op state machine, all mutators must be idle
// TODO: every callback must be for the current op and market index

// TODO: supply queue must never have duplicates

// TODO: fee accruel must only happen when AUM grows

// Allocations
// TODO: on allocation-failure, reconcile to idle
// TODO: allocation accounting: Accepted amount = new_principal - before &never more than attempted
// TODO: allocation attempts: any market that is enabled (new_principal > 0) must be in the withdraw queue

// Withdraws
// TODO: try withdraw & idle first: idle balance can be utilised on a first-come-first-serve basis => it
// is **not** deducted until payout succeeds
// TODO: create withdraw: if create withdraw fails, skip to next market
// TODO: execute withdraw: if executing a withdrawal fails, assume nothing changed
// TODO: withdrawn(execute > read): withdrawn credits must increase idle balance
// TODO: withdraw queue must never have duplicates
// TODO: enabling a market (cap > 0) must add it to the withdraw queue

// TODO: Skim: is no-op when balance is 0

// Payouts
// TODO: payout success: idle balance must decrease & burn escrowed shares
// TODO: payout failure: idle doesnt change  & refund escrowed shares to original owner

// TODO: stop and exit: must never mutiny funds or escrow

// TODO: credit principal only after proper supply to marfket

// TODO: Withdraw read onlky credits idle

// TODO: on error, assume no risk
//
//
//

// TODO: test harness
// We need:
// - market setup (using market::setup_test)
// - vault version of setup_test, utilising the market principal
