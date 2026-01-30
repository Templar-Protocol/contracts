# Vault Parity Tests

This document describes how to run property tests and verify parity across the kernel, NEAR, and Soroban vault implementations.

## Architecture Overview

The vault implementation follows a kernel + executor pattern:

```
┌─────────────────────────────────────────────────────────────────┐
│                         Kernel Crate                            │
│  (templar-vault-kernel)                                         │
│  - Pure state machine logic                                     │
│  - Chain-agnostic property tests                                │
│  - Formal verification harnesses (Kani)                         │
└─────────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┴───────────────┐
              ▼                               ▼
┌─────────────────────────┐   ┌─────────────────────────┐
│   NEAR Executor         │   │   Soroban Executor      │
│ (templar-vault-contract)│   │ (templar-soroban-vault) │
│ - NEAR-specific storage │   │ - Soroban-specific      │
│ - Borsh serialization   │   │   storage & auth        │
│ - Integration tests     │   │ - Parity property tests │
│ - Gas profiling         │   │ - Integration tests     │
└─────────────────────────┘   └─────────────────────────┘
```

## Running Property Tests

### Kernel Property Tests (Source of Truth)

The kernel contains 70+ property-based tests that verify core invariants:

```bash
# Run all kernel property tests
cargo test -p templar-vault-kernel --test property_tests

# Run specific property categories
cargo test -p templar-vault-kernel --test property_tests prop_accounting
cargo test -p templar-vault-kernel --test property_tests prop_queue
cargo test -p templar-vault-kernel --test property_tests prop_fee
cargo test -p templar-vault-kernel --test property_tests prop_conversion
```

Key invariants tested:
- **Accounting**: `total_assets = idle_assets + external_assets`
- **Queue**: FIFO ordering, length bounds, head monotonicity
- **Fees**: Non-negative accrual, zero fee → zero shares
- **Conversions**: Roundtrip bounds, monotonicity, ERC4626 consistency

### Soroban Parity Tests

The Soroban executor includes property tests that verify parity with the kernel:

```bash
# Run Soroban property tests (parity verification)
cargo test --manifest-path contract/vault/soroban/Cargo.toml --test property_tests

# Run Soroban integration tests
cargo test --manifest-path contract/vault/soroban/Cargo.toml --test integration_tests

# Run Soroban flow tests
cargo test --manifest-path contract/vault/soroban/Cargo.toml --test flows
```

The Soroban parity tests verify:
- Accounting invariant holds after operations
- State machine transitions match kernel behavior
- Effect generation is consistent

### NEAR Integration Tests

NEAR uses integration tests with `near-workspaces`:

```bash
# Run NEAR integration tests
cargo test -p templar-vault-contract

# Run specific test
cargo test -p templar-vault-contract --test happy_path
```

## Running All Parity Tests

To verify parity across all implementations:

```bash
#!/usr/bin/env bash
set -e

echo "=== Running Kernel Property Tests ==="
cargo test -p templar-vault-kernel --test property_tests

echo "=== Running Kernel Kani Proofs (test equivalents) ==="
cargo test -p templar-vault-kernel --test kani_proofs

echo "=== Running Soroban Parity Tests ==="
cargo test --manifest-path contract/vault/soroban/Cargo.toml --test property_tests

echo "=== Running Soroban Integration Tests ==="
cargo test --manifest-path contract/vault/soroban/Cargo.toml --test integration_tests

echo "=== Running NEAR Integration Tests ==="
cargo test -p templar-vault-contract

echo "=== All parity tests passed ==="
```

## Formal Verification (Kani)

The kernel includes Kani proof harnesses for critical invariants:

```bash
# Run Kani proofs (requires Kani installation)
cargo kani --tests -p templar-vault-kernel

# Run test equivalents when Kani is not available
cargo test -p templar-vault-kernel --test kani_proofs
```

## NEAR Gas Delta Check

Monitor gas usage changes against a baseline:

```bash
# Run gas delta check (compares against baseline)
./scripts/gas_delta_check.sh

# With custom threshold (default: 10%)
./scripts/gas_delta_check.sh --threshold 15

# Generate new baseline (updates gas_baseline.json)
cargo run --example gas_report -p templar-vault-contract
```

The baseline is stored in `contract/vault/near/gas_baseline.json`.

### Interpreting Gas Results

| Action           | Typical Gas | Description |
|------------------|-------------|-------------|
| `supply`         | ~8.2 Tgas   | Deposit assets, mint shares |
| `allocate`       | ~20.7 Tgas  | Allocate idle to market |
| `withdraw`       | ~4.4 Tgas   | Request withdrawal |
| `execute_withdraw` | ~10.0 Tgas | Execute pending withdrawal |
| `submit_cap`     | ~2.7 Tgas   | Submit allocation cap |

## Property Test Categories

### Shared Properties (Kernel)

| Category | Properties | Description |
|----------|------------|-------------|
| Accounting | 10 | Total assets = idle + external |
| Queue | 15 | FIFO, length bounds, status |
| Conversion | 10 | Share/asset roundtrips |
| Fees | 10 | Non-negative, bounded, monotonic |
| State Machine | 15 | Transition guards, op ID matching |
| Escrow | 10 | Settlement conservation |

### Parity Properties (Soroban)

| Property | Verified Against |
|----------|------------------|
| `prop_accounting_invariant` | Kernel accounting rules |
| `prop_roundtrip_bounded` | Kernel conversion logic |
| `prop_state_machine_completes` | Kernel transitions |
| `prop_effects_consistent` | Kernel effect generation |

## Adding New Parity Tests

1. Add property to kernel (`property_tests.rs`)
2. Add equivalent test to Soroban (`property_tests.rs`)
3. Verify NEAR behavior through integration tests

Example kernel property:
```rust
proptest! {
    #[test]
    fn prop_new_invariant(
        assets in 1u128..=1_000_000_000u128,
        shares in 1u128..=1_000_000_000u128,
    ) {
        // Property assertion
    }
}
```

## CI Integration

The property tests run in CI via:
- `cargo test -p templar-vault-kernel` (kernel + properties)
- `cargo test -p templar-vault-contract` (NEAR integration)
- `cargo test --manifest-path contract/vault/soroban/Cargo.toml` (Soroban)

Gas delta checks are manual but can be added to CI with:
```yaml
- name: Gas delta check
  run: ./scripts/gas_delta_check.sh --threshold 10
```
