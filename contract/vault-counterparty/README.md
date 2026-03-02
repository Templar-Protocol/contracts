# Vault Counterparty Spike (NEAR)

This crate is a spike for the HOT Bridge counterparty described in the Stellar <-> NEAR design note.

## What this verifies

- Curator-only methods build the expected outbound calls to the configured `omni_contract`:
  - `mt_transfer(receiver_id, token_id, amount)`
  - `withdraw(token_id, receiver_id, amount)`
- Each outbound call attaches exactly `1 yoctoNEAR`.
- `mt_transfer` always uses the preconfigured `near_market` receiver from init.
- `withdraw_to_stellar` always uses the contract's configured `stellar_receiver`.
- `token_id` is immutable (`omni_token_id`) from init; runtime methods do not accept token overrides.
- omni contract is configurable at init (`omni_contract`) to allow bridge transport swaps.
- No runtime callback `msg` input is exposed in this spike (no arbitrary action payloads).
- Security guards in the spike contract:
  - Reject `amount == 0`
  - Reject empty `token_id` at initialization
  - Cap `token_id` and `stellar_receiver` length
  - Two-step owner transfer (`propose_owner` -> `accept_owner`)

The tests inspect emitted mock receipts directly, so method names, JSON arg shapes, gas, and deposit are all checked.

## Soroban adapter compatibility

`stellar_receiver` is a plain string provided at init. For this spike, that lets you choose either:

- A real Soroban adapter address (for example from `upstream/feat/soroban-curator-kernel`)
- A mock placeholder receiver during local testing

`near_market` and `omni_token_id` are set at init and immutable in this spike, so curator cannot route to arbitrary receivers or switch bridged assets at call time. `omni_contract` is also set at init so the same contract can target a non-HOT multitoken contract in the future.

## Run

```bash
cargo test -p templar-vault-counterparty-contract
```
