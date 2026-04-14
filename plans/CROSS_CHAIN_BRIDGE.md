# Cross-Chain Bridge: Stellar <-> NEAR (HOT First, Swappable Transport)

## Decision

Ship with HOT Bridge now, but keep transport swappable.

## Addendum: Transport Abstraction

### Goals

- Ship quickly on HOT while it is live and working.
- Avoid lock-in if HOT Stellar support degrades.
- Keep vault + market contracts stable across transport swaps.

### Contract Boundaries

- NEAR counterparty is a verifier and router guardrail, not a bridge implementation.
- Counterparty has immutable init config:
  - `stellar_receiver`
  - `near_market`
  - `omni_token_id`
  - `omni_contract`
- Runtime methods do not accept arbitrary receiver/token/action payload overrides.
- The Stellar bridge path should present as a market-like adapter to vault curation flows.
- From the curator/allocator point of view, a "deposit" into that adapter means routing funds into the bridge counterparty path, not depositing into a real yield market on Stellar.
- Operationally, the adapter "pretends" to be a market while actually transferring assets on NEAR into the counterparty/bridge route.

### Relayer Boundary

Introduce a bridge-transport interface in the relayer service:

- `BridgeRelayer::complete_deposit(event)`
- `BridgeRelayer::complete_withdrawal(pending)`

`HotBridgeRelayer` is the first implementation.

If transport changes later, implement a new relayer adapter behind the same trait and keep the loop/orchestration code stable.

## What Changes on Transport Swap

- Deploy new Stellar adapter for the new bridge locker/prover model.
- Deploy or point counterparty to new `omni_contract`.
- Switch relayer transport implementation.

## What Does Not Change

- Templar market contracts.
- Vault contract logic.
- Counterparty RBAC model and immutable routing constraints.

## Operational Notes

- Mainnet-only verification is acceptable with tiny amounts due to no public HOT testnet.
- Confirm MPC API access/limits with HOT team.
- Keep monitoring and fallback runbooks for relayer liveness.
- Verified via live bridge API queries for `carrion256.near`: Stellar deposit routing is currently stable across repeated requests, returning the same deposit address `GDJ4JZXZELZD737NVFORH4PSSQDWFDZTKW3AIDKHYQG23ZXBPDGGQBJK` and memo `49425867` on consecutive calls.
- For the current spike, the Stellar-side integration can treat that HOT locker address + memo pair as effectively static for `carrion256.near`, while preserving the option to re-query if HOT changes routing behavior later.

## Verified Mainnet Transaction Trail

- Stellar proof-of-control transfer from the generated test account back to the funder:
  - tx: `4dbb21cbed36c30eb7620d70524f7d3acf73eee71ed443fc4fde61cc537dbd1f`
  - result: successful 1 XLM payment from `GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV` to `GD3SOHKDS7CDGDOTJKP6VNAOEXC3Y5BRWD3WIEK65ZQAJUMTBGE4TVBZ`
- NEAR proof-of-control transfer from the implicit account back to `carrion256.tg`:
  - tx: `6hrJEufLstUX3KuHsCUaU9YYMYCtZnTpSQpLzNLBaeoM`
  - result: successful 0.9 NEAR transfer from `abf8a4e3650dd797a8bce5cb46b22fa791473f6fb17c5e6e4584e301cb8fffd6` to `carrion256.tg`
- NEAR named account creation for the counterparty:
  - tx: `CkAQwwt3vrfcbuPqXQ2mpDqvVaAVdDZUKyXtJWGNY5wm`
  - result: `carrion256.near` created on mainnet
- NEAR funding transfer into `carrion256.near` for bootstrap:
  - tx: `2i6U5G2ZFSDYSYncKck39uqGRkrpxVu9rLyzyLkdh8Ha`
  - result: successful 5 NEAR transfer into `carrion256.near`
- Counterparty contract deploy to `carrion256.near` with corrected `cargo near` artifact:
  - tx: `6FWCW3ErGaH2DmgsKFigGkMAXSc6f182mWaLP5eiRtyr`
  - result: contract code deployed successfully
- Counterparty contract initialization on NEAR:
  - tx: `AnjCXStSps8Exe6VjkhY5NnjhQK9ZutMVdKke2MuTbsw`
  - result: config stored for `stellar_receiver`, `near_market`, `omni_token_id`, `omni_contract`, `owner`, and `curator`
- HOT omni storage registration for `ixlm-ixlmusdc.v1.tmplr.near`:
  - completed via signed transaction broadcast after near-cli-rs nonce issues
  - verification: `storage_balance_of("ixlm-ixlmusdc.v1.tmplr.near")` returned present on-chain
- HOT omni storage registration for `carrion256.near`:
  - completed via fresh signed transaction broadcast after nonce collision recovery
  - verification: `storage_balance_of("carrion256.near")` returned present on-chain
- Stellar HOT locker deposit of native XLM into HOT's Soroban locker:
  - tx: `56876328c88bd52755fa4cc3346442e9bace1adf3e962c57d5a0cddb719b9615`
  - contract: `CCLWL5NYSV2WJQ3VBU44AMDHEVKEPA45N2QP2LL62O3JVKPGWWAQUVAG`
  - token contract: `CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA`
  - amount: `1000000` stroops (`0.1 XLM`)
  - result: successful locker deposit from `GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV`
  - returned nonce: `1776173770273628119000`
- HOT bridge completion into NEAR:
  - tx: `2TTZViJT3zBHkt1LaoADJZf56ouS9yBZcaZUqa3xmaQw`
  - sender: `0here.tg`
  - result: final on NEAR
  - on-chain effects:
    - `v2_1.omni.hot.tg` minted `1100_111bzQBB5v7AhLyPMDwS8uJgQV24KaAPXtwyVWu2KXbbfQU6NXRCz`
    - `v2_1.omni.hot.tg` transferred that amount to `intents.near`
    - `intents.near` minted `nep245:v2_1.omni.hot.tg:1100_111bzQBB5v7AhLyPMDwS8uJgQV24KaAPXtwyVWu2KXbbfQU6NXRCz` to `carrion256.near`
    - minted amount on `carrion256.near`: `1000000`
