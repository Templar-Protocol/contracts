# Vault

This directory contains shared vault runtime/testing material for the kernel, NEAR executor, and Soroban executor.

## Architecture

The vault system follows a kernel + executor split:

- `templar-vault-kernel` is the chain-agnostic source of truth for state transitions, math, and invariants.
- `contract/vault/near` executes kernel behavior on NEAR (storage, callbacks, token interfaces, gas-specific concerns).
- `contract/vault/soroban` executes kernel behavior on Soroban (storage/auth wiring and sync execution model).
- `contract/vault/curator-primitives` holds shared policy/recovery helpers used by executors.

```mermaid
graph TB
    subgraph Kernel["templar-vault-kernel"]
        APPLY["apply_action(state, config, restrictions, self_id, action)"]
        VSTATE["VaultState\ntotal_assets\ntotal_shares\nidle_assets\nexternal_assets\nfee_anchor\nop_state\nwithdraw_queue\nnext_op_id"]
        OPSTATE["OpState\nIdle / Allocating / Withdrawing / Refreshing / Payout"]
        KEFFECTS["KernelEffect\nMint/Burn/Transfer shares\nTransfer assets\nEmitEvent"]
        KMATH["Math + fees\nshare/asset conversion\nfee accrual"]
        APPLY --> VSTATE
        APPLY --> OPSTATE
        APPLY --> KEFFECTS
        APPLY --> KMATH
    end

    subgraph Primitives["contract/vault/curator-primitives"]
        POLICY["PolicyState\nsupply queue\ncap groups\nmarket locks"]
        AUTH["ActionKind + AuthPolicyClass\nPublic / Sentinel / Allocator\nAllocatorEmergency / Curator"]
        GOV["Governance + recovery helpers"]
    end

    subgraph Executors["Runtime executors"]
        NEAR["contract/vault/near\nNEAR storage + callbacks\nexecutor integration"]

        subgraph Soroban["contract/vault/soroban"]
            ENTRY["SorobanVaultContract\nentrypoints"]
            CVAULT["CuratorVault<S, A, E>\nexecutor orchestration"]
            STORAGE["SorobanStorage\nversioned state blob\nTTL extension + migration gate"]
            RBAC["RbacAuth / Soroban auth wiring"]
            INTERP["SorobanEffectInterpreter\nSEP-41 + token transfers\npostcard kernel events"]
            ADDR["kernel_address_from_sdk()\nSHA256(domain || strkey)"]

            ENTRY --> CVAULT
            CVAULT --> STORAGE
            CVAULT --> RBAC
            CVAULT --> INTERP
            CVAULT --> ADDR
        end
    end

    Primitives --> Soroban
    Primitives --> NEAR
    Kernel --> Soroban
    Kernel --> NEAR
```

### Current Soroban Flow

The Soroban executor is the most feature-complete runtime today. It loads a
versioned state blob, maps Soroban addresses into kernel addresses, dispatches a
`KernelAction`, executes the returned `KernelEffect`s, and then persists the new
state.

```mermaid
sequenceDiagram
    actor Caller
    participant Contract as SorobanVaultContract
    participant Vault as CuratorVault
    participant Kernel as apply_action()
    participant Effects as SorobanEffectInterpreter
    participant Asset as Asset token
    participant Share as Share token

    Caller->>Contract: deposit_with_min / request_withdraw / execute_withdraw
    Contract->>Contract: require_auth()
    Contract->>Vault: load storage + config + RBAC
    Vault->>Vault: map SDK address -> kernel address
    Vault->>Kernel: apply_action(..., KernelAction)
    Kernel-->>Vault: new state + KernelEffect[]
    Vault->>Effects: execute_effects()
    Effects->>Asset: transfer / transfer_from
    Effects->>Share: mint / burn / transfer
    Effects-->>Contract: emit postcard kernel event(s)
    Contract-->>Caller: result
```

### Kernel Operation State Machine

```mermaid
stateDiagram-v2
    [*] --> Idle

    Idle --> Allocating: BeginAllocating
    Idle --> Withdrawing: ExecuteWithdraw
    Idle --> Refreshing: BeginRefreshing

    Allocating --> Idle: FinishAllocating
    Allocating --> Idle: AbortAllocating
    Allocating --> Idle: EmergencyReset

    Withdrawing --> Withdrawing: RebalanceWithdraw / collect more liquidity
    Withdrawing --> Payout: withdrawal_settled(...)
    Withdrawing --> Idle: AbortWithdrawing
    Withdrawing --> Idle: EmergencyReset

    Refreshing --> Idle: FinishRefreshing
    Refreshing --> Idle: AbortRefreshing
    Refreshing --> Idle: EmergencyReset

    Payout --> Idle: SettlePayout(Success|Failure)
    Payout --> Idle: EmergencyReset
```

### Public Withdrawal Lifecycle

```mermaid
sequenceDiagram
    actor User
    participant Contract as SorobanVaultContract
    participant Vault as CuratorVault
    participant Kernel as apply_action()
    participant Share as Share token
    participant Asset as Asset token

    User->>Contract: request_withdraw(owner, receiver, shares, min_assets_out)
    Contract->>Vault: request_withdraw(...)
    Vault->>Kernel: KernelAction::RequestWithdraw
    Kernel-->>Vault: queue request + TransferShares(owner -> escrow) + EmitEvent
    Vault->>Share: transfer shares into escrow
    Contract-->>User: request_id

    Note over User: wait until cooldown is satisfied

    User->>Contract: execute_withdraw(caller)
    Contract->>Vault: execute_withdraw(...)
    Vault->>Kernel: KernelAction::ExecuteWithdraw
    Kernel-->>Vault: transition Idle -> Withdrawing
    alt enough idle liquidity
        Vault->>Vault: complete_withdrawal_from_idle()
        Vault->>Kernel: withdrawal_settled(...)
        Vault->>Asset: transfer assets to receiver
        Vault->>Kernel: KernelAction::SettlePayout
        Vault->>Share: burn escrowed shares / refund remainder if needed
    else allocator must free market liquidity first
        Note over Vault: allocator path uses BeginAllocating + RebalanceWithdraw\nand later re-runs execute_withdraw
    end
    Contract-->>User: ok
```

### Authorization Model

Canonical action policy lives in `contract/vault/curator-primitives/src/auth/mod.rs`.
The Soroban runtime enforces the same action classes through `RbacAuth` and
`require_auth()`.

```mermaid
graph TD
    Public["Public\nDeposit\nRequestWithdraw\nAtomicWithdraw"]
    Sentinel["Sentinel\nPause\nSetRestrictions"]
    Allocator["Allocator\nExecuteWithdraw\nBegin/FinishAllocating\nSyncExternalAssets\nRebalanceWithdraw\nBegin/FinishRefreshing\nSettlePayout\nRefreshFees"]
    AllocEmergency["AllocatorEmergency\nAbortAllocating\nAbortWithdrawing\nAbortRefreshing"]
    Curator["Curator\nPolicyAdmin\nManualReconcile\nEmergencyReset"]

    Curator --> Allocator
    Sentinel --> AllocEmergency
    Allocator --> AllocEmergency
    AllocEmergency --> Public
    Allocator --> Public
    Sentinel --> Public
    Curator --> Public
```

## Parity Tests

Parity tests verify behavioral equivalence across the kernel and executors.

## Test and Verification Recipes

Use the vault recipe index in [contract/vault/justfile](./justfile).

If you run from repo root, call recipes as:
- `just -f contract/vault/justfile <recipe>`

Core recipes:
- Kernel test suite: `kernel-test`
- Kernel property tests: `kernel-prop`
- Curator primitives tests: `curator-test`
- Curator primitives property tests: `curator-prop`
- NEAR integration tests: `near-test`
- Soroban unit/integration recipes: `soroban-test`, `soroban-prop`, `soroban-integration`
- Cross-surface parity run: `parity`
- Full vault test sweep: `vault-test`
- Gas reporting: `gas-report`

Soroban runtime/deployment workflows are in [contract/vault/soroban/justfile](./soroban/justfile).

Gas baselines are stored in `contract/vault/near/gas_baseline.json`.

### Interpreting Gas Results

| Action             | Typical Gas | Description                     |
|--------------------|-------------|---------------------------------|
| `supply`           | ~8.2 Tgas   | Deposit assets, mint shares     |
| `allocate`         | ~20.7 Tgas  | Allocate idle to market         |
| `withdraw`         | ~4.4 Tgas   | Request withdrawal              |
| `execute_withdraw` | ~10.0 Tgas  | Execute pending withdrawal      |
| `submit_cap`       | ~2.7 Tgas   | Submit allocation cap           |

## Property Test Categories

### Shared Properties (Kernel)

| Category      | Properties | Description                         |
|---------------|------------|-------------------------------------|
| Accounting    | 10         | Total assets = idle + external      |
| Queue         | 15         | FIFO, length bounds, status         |
| Conversion    | 10         | Share/asset roundtrips              |
| Fees          | 10         | Non-negative, bounded, monotonic    |
| State Machine | 15         | Transition guards, op ID matching   |
| Escrow        | 10         | Settlement conservation             |

### Parity Properties (Soroban)

| Property                        | Verified Against          |
|---------------------------------|---------------------------|
| `prop_accounting_invariant`     | Kernel accounting rules   |
| `prop_roundtrip_bounded`        | Kernel conversion logic   |
| `prop_state_machine_completes`  | Kernel transitions        |
| `prop_effects_consistent`       | Kernel effect generation  |

## Adding New Parity Tests

1. Add property to kernel (`property_tests.rs`)
2. Add equivalent test to Soroban (`property_tests.rs`)
3. Validate with justfile recipes: `kernel-prop`, `soroban-prop`, `near-test`

## CI Integration

CI should invoke the same justfile recipes used locally (`kernel-prop`, `near-test`, `soroban-prop`).

## Security Docs

- Soroban STRIDE threat model: `contract/vault/soroban/STRIDE.md`
- Soroban runtime operational notes: `contract/vault/soroban/README.md`
