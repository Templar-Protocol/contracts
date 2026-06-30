# Custodial Adapter Deployment Guide

This guide explains how to deploy your own **custodial adapter** — the Soroban contract that sits between the Templar vault and an offchain custodian, forwards allocated funds, and tracks reported route NAV.

> **Source:** [`contract/vault/soroban/custodial-adapter/src/lib.rs`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/soroban/custodial-adapter/src/lib.rs)

---

## Roles

| Role | Description |
|---|---|
| **`admin`** | Governance address. Can pause/unpause the adapter, propose a new admin, upgrade the contract, and override NAV during recovery. May be an EOA or a governance contract. |
| **`vault`** | The vault contract this adapter reports to. Only the vault may call `supply`, `withdraw`, and `progress_withdrawal`. |
| **`custodian`** | Receives allocated funds from the adapter and is expected to return them before withdrawals are progressed. May be an EOA or a contract. Can report NAV updates via `set_reported_assets`. |
| **`asset`** | The single Stellar Asset Contract (SAC) this adapter manages. |

---

## Step 1 — Build the WASM

```bash
cd contract/vault/soroban/custodial-adapter
stellar contract build
```

Or using Cargo directly:

```bash
cargo build --target wasm32-unknown-unknown --release
```

---

## Step 2 — Upload the WASM

```bash
stellar contract upload \
  --wasm target/wasm32-unknown-unknown/release/custodial_adapter.wasm \
  --source <your-keypair> \
  --network mainnet
```

This returns a `WASM_HASH` used in the next step.

---

## Step 3 — Deploy and initialise

The constructor takes four arguments:

```bash
stellar contract deploy \
  --wasm-hash <WASM_HASH> \
  --source <your-keypair> \
  --network mainnet \
  -- \
  --admin    <ADMIN_ADDRESS> \
  --vault    <VAULT_CONTRACT_ADDRESS> \
  --custodian <CUSTODIAN_ADDRESS> \
  --asset    <ASSET_CONTRACT_ADDRESS>
```

The command prints the new **contract ID** — save it.

### Constructor constraints

The constructor rejects invalid configurations with `InvalidInput` (error code `#2`):

| Constraint | Reason |
|---|---|
| `vault` must be a contract address | Account (G…) addresses are rejected |
| `asset` must be a contract address | Account addresses are rejected |
| No argument may equal the adapter's own address | Prevents self-reference |
| `asset` ≠ `admin`, `vault`, or `custodian` | Prevents role/asset aliasing |
| `custodian` ≠ `vault` | Enforces separation of concerns |

---

## Step 4 — Report initial NAV (optional)

`reported_assets` starts at `0` after deployment. The vault auto-increments it when `supply` is called. If you need to seed a non-zero NAV before the vault supplies funds, call `set_reported_assets` as the custodian:

```bash
stellar contract invoke \
  --id      <ADAPTER_CONTRACT_ID> \
  --source  <custodian-keypair> \
  --network mainnet \
  -- set_reported_assets \
  --caller           <CUSTODIAN_ADDRESS> \
  --asset            <ASSET_ADDRESS> \
  --expected_current 0 \
  --amount           <INITIAL_NAV> \
  --report_nonce     1
```

`report_nonce` must always be exactly `current_nonce + 1`. Every NAV-mutating operation (`supply`, `withdraw`, `progress_withdrawal`, `set_reported_assets`) advances the nonce by one, so stale or replayed reports are rejected.

---

## Post-deployment operations

### Pause / unpause

Either the admin or the vault may pause the adapter:

```bash
stellar contract invoke \
  --id <ADAPTER_CONTRACT_ID> \
  --source <admin-or-vault-keypair> \
  --network mainnet \
  -- set_paused \
  --caller <ADMIN_OR_VAULT_ADDRESS> \
  --paused true
```

While paused:
- `supply` is blocked.
- `set_reported_assets` is blocked for the vault and custodian.
- `progress_withdrawal` and `withdraw` **remain available** so already-returned liquidity can be recovered.
- The admin can still call `set_reported_assets` for emergency NAV correction.

### Transfer admin

Admin transfer is a two-step process to prevent locking out accidentally:

```bash
# Step 1 — current admin proposes a new admin
stellar contract invoke --id <ADAPTER_CONTRACT_ID> --source <admin-keypair> \
  --network mainnet -- set_admin \
  --caller <CURRENT_ADMIN> --admin <NEW_ADMIN>

# Step 2 — new admin accepts
stellar contract invoke --id <ADAPTER_CONTRACT_ID> --source <new-admin-keypair> \
  --network mainnet -- accept_admin \
  --caller <NEW_ADMIN>
```

### Upgrade the contract

Only the admin may upgrade:

```bash
stellar contract invoke \
  --id <ADAPTER_CONTRACT_ID> \
  --source <admin-keypair> \
  --network mainnet \
  -- upgrade \
  --new_wasm_hash <NEW_WASM_HASH> \
  --operator      <ADMIN_ADDRESS>
```

### Extend instance TTL

The adapter extends its own TTL on every call. Anyone may also trigger this explicitly at no permission cost:

```bash
stellar contract invoke \
  --id <ADAPTER_CONTRACT_ID> \
  --source <any-keypair> \
  --network mainnet \
  -- extend_ttl
```

---

## Error reference

| Code | Name | Meaning |
|---|---|---|
| `#1` | `Unauthorized` | Caller is not permitted for this operation |
| `#2` | `InvalidInput` | Bad argument or constraint violation |
| `#3` | `MissingConfig` | Required storage key not set (e.g. no pending admin) |
| `#4` | `ArithmeticOverflow` | NAV addition would overflow `i128` |
| `#5` | `ArithmeticUnderflow` | NAV subtraction would go negative |
| `#6` | `InsufficientReturnedLiquidity` | Not enough idle balance on the adapter to satisfy the withdrawal |
| `#7` | `Paused` | Operation blocked while the adapter is paused |
