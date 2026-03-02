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
