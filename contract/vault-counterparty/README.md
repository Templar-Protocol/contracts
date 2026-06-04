# Vault Counterparty Spike (NEAR)

This crate is a spike for the HOT Bridge counterparty described in the Stellar <-> NEAR design note.

## What this verifies

- Curator-only methods build the expected outbound calls to the configured `omni_contract`:
  - `mt_transfer_call(receiver_id, token_id, amount, msg)` with `msg = "\"Supply\""` for market deposits
  - `intents.near mt_withdraw(token, receiver_id, token_ids, amounts, msg)` for HOT withdrawals
- The market-like adapter entrypoint accepts vault supply through `intents.near mt_transfer_call`
  with token id `nep245:<omni_contract>:<raw_hot_token_id>` and `msg = "\"Supply\""`.
- HOT asset transfer calls attach exactly `1 yoctoNEAR`; market queue calls attach no deposit.
- `forward_to_market` always uses the preconfigured `near_market` receiver from init and credits
  the counterparty's market supply position.
- Curator-only market withdrawal helpers let the counterparty enter, cancel, and execute the
  market supply withdrawal queue before returned assets are withdrawn to Stellar.
- `withdraw_to_stellar` always uses the contract's configured `stellar_receiver` and only spends
  the counterparty's current Intents balance.
- `token_id` is immutable (`omni_token_id`) from init; runtime methods do not accept token overrides.
- omni contract is configurable at init (`omni_contract`) to allow bridge transport swaps.
- `hot_deposit_receiver_hex` is explicit init config. It must come from the verified HOT receiver
  bytes instead of local recomputation.
- No runtime callback `msg` input is exposed in this spike (no arbitrary action payloads).
- Security guards in the spike contract:
  - Reject `amount == 0`
  - Reject empty `token_id` at initialization
  - Cap `token_id` and `stellar_receiver` length
  - Two-step owner transfer (`propose_owner` -> `accept_owner`)

The tests inspect emitted mock receipts directly, so method names, JSON arg shapes, gas, and deposit are all checked.

## Market exit flow

When funds have been forwarded to the market, they must be brought back to the counterparty before
`withdraw_to_stellar` can bridge them out:

1. `request_market_withdrawal(amount)` enters the counterparty's market supply position into the
   withdrawal queue.
2. `execute_market_withdrawal(batch_limit)` advances the market withdrawal queue and transfers
   available borrow assets back to the counterparty.
3. `withdraw_to_stellar(amount)` withdraws the returned Intents balance through HOT.

When the vault uses this contract as the Stellar market adapter, the flow is:

1. Vault allocation calls `intents.near mt_transfer_call` into this adapter.
2. The adapter records a vault supply position and calls `intents.near mt_withdraw` to hand the
   wrapped HOT asset to `bridge-refuel.hot.tg`.
3. Vault withdrawal calls the adapter's market withdrawal queue methods.
4. Once HOT has returned assets to the adapter's Intents balance, `execute_next_supply_withdrawal_request`
   transfers the Intents-wrapped token back to the vault.

## Soroban adapter compatibility

`stellar_receiver` is a plain string provided at init. For this spike, that lets you choose either:

- A real Soroban adapter address (for example from `upstream/feat/soroban-curator-kernel`)
- A mock placeholder receiver during local testing

`near_market` and `omni_token_id` are set at init and immutable in this spike, so curator cannot route to arbitrary receivers or switch bridged assets at call time. `omni_contract` is also set at init so the same contract can target a non-HOT multitoken contract in the future.

## Run

```bash
cargo test -p templar-vault-counterparty-contract
```

## Operations

Use the local justfile for deployment, bridge smoke flows, and HOT/Stellar helper operations:

```bash
just -f contract/vault-counterparty/justfile help
just -f contract/vault-counterparty/justfile stellar-deposit <amount>
just -f contract/vault-counterparty/justfile monitor-withdrawal <nonce>
```

If a tool needs `STELLAR_SECRET_KEY` instead of a Stellar CLI identity name, load it from the local
Stellar keystore without printing it:

```bash
source contract/vault-counterparty/scripts/load-stellar-secret-env.sh
```

Do not use funding-bridge scripts for counterparty or HOT bridge operations; this crate owns the
counterparty deployment and Stellar locker helper flow.
