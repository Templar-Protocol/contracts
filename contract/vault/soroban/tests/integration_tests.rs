//! Integration tests for the Soroban curator vault.
//!
//! These tests verify full flows: deposit -> allocate -> refresh -> withdraw.

use rstest::{fixture, rstest};
use soroban_sdk::{testutils::Address as _, Env};
use templar_soroban_runtime::{
    contract::{ContractConfig, CuratorVault, SorobanVaultContract},
    rbac::{RbacAuth, RbacConfig, Role},
    storage::{MemoryStorage, SorobanStorage, VersionedState},
    EffectContext,
    EffectInterpreter,
    Storage, // Import the trait
};
use templar_vault_kernel::state::queue::DEFAULT_COOLDOWN_NS;
use templar_vault_kernel::{
    apply_action, effects::KernelEffect, Address, AllocatingState, FeesSpec, KernelAction, OpState,
    VaultConfig, VaultState, MAX_PENDING, MIN_WITHDRAWAL_ASSETS,
};
use templar_vault_kernel::{
    state::op_state::RefreshingState,
    transitions::{
        allocation_step_callback, complete_allocation, complete_refresh, payout_complete,
        refresh_step_callback, start_allocation, start_refresh, start_withdrawal,
        withdrawal_collected, withdrawal_step_callback, WithdrawalRequest,
    },
};

mod common;
use common::{MockInterpreter, TestPermissiveAuth};

// Test Helpers

fn test_config() -> ContractConfig {
    ContractConfig::new(
        [1u8; 32],       // curator
        [9u8; 32],       // vault_address
        vec![[2u8; 32]], // guardians
        vec![[3u8; 32]], // allocators
        [4u8; 32],       // asset_address
        [5u8; 32],       // share_address
    )
}

fn curator_addr() -> Address {
    [1u8; 32]
}

fn guardian_addr() -> Address {
    [2u8; 32]
}

fn sentinel_addr() -> Address {
    [11u8; 32]
}

fn allocator_addr() -> Address {
    [3u8; 32]
}

fn user_addr() -> Address {
    [10u8; 32]
}

struct SorobanContractFixture {
    env: Env,
    contract_id: soroban_sdk::Address,
}

#[fixture]
fn soroban_contract_fixture() -> SorobanContractFixture {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator.clone(), curator, asset, share)
            .unwrap();
    });

    SorobanContractFixture { env, contract_id }
}

#[rstest]
fn soroban_contract_vault_snapshot_matches_fields(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;

    env.as_contract(&contract_id, || {
        let (total_shares, idle_assets, external_assets) =
            SorobanVaultContract::vault_snapshot(env.clone()).unwrap();
        assert_eq!(total_shares, 0);
        assert_eq!(idle_assets, 0);
        assert_eq!(external_assets, 0);
    });
}

fn preview_kernel_config(paused: bool) -> VaultConfig {
    VaultConfig {
        fees: FeesSpec::zero(),
        min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
        withdrawal_cooldown_ns: DEFAULT_COOLDOWN_NS,
        max_pending_withdrawals: MAX_PENDING as u32,
        paused,
        virtual_shares: 0,
        virtual_assets: 0,
    }
}

fn mint_shares_from_deposit(state: VaultState, assets_in: u128) -> u128 {
    let owner = [1u8; 32];
    let receiver = [2u8; 32];
    let self_id = [9u8; 32];
    let config = preview_kernel_config(false);
    let result = apply_action(
        state,
        &config,
        None,
        &self_id,
        KernelAction::Deposit {
            owner,
            receiver,
            assets_in,
            min_shares_out: 0,
            now_ns: 1,
        },
    )
    .expect("kernel deposit");
    result
        .effects
        .iter()
        .find_map(|effect| match effect {
            templar_vault_kernel::effects::KernelEffect::MintShares { shares, .. } => Some(*shares),
            _ => None,
        })
        .expect("mint shares effect")
}

#[rstest]
fn soroban_contract_preview_deposit_matches_kernel(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let assets_in = 500u128;

    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        let empty_state = VaultState::default();
        let versioned = VersionedState::new(empty_state.clone());
        storage.save_state(&versioned).unwrap();

        let preview =
            SorobanVaultContract::preview_deposit(env.clone(), assets_in as i128).unwrap();
        let minted = mint_shares_from_deposit(empty_state, assets_in);
        assert_eq!(preview as u128, minted);
    });

    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        let mut state = VaultState::default();
        state.total_assets = 10_000;
        state.total_shares = 8_000;
        state.idle_assets = 10_000;
        let versioned = VersionedState::new(state.clone());
        storage.save_state(&versioned).unwrap();

        let preview =
            SorobanVaultContract::preview_deposit(env.clone(), assets_in as i128).unwrap();
        let minted = mint_shares_from_deposit(state, assets_in);
        assert_eq!(preview as u128, minted);
    });
}

#[rstest]
fn soroban_contract_preview_withdraw_matches_kernel(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        let mut state = VaultState::default();
        state.total_assets = 20_000;
        state.total_shares = 12_000;
        state.idle_assets = 20_000;
        let versioned = VersionedState::new(state.clone());
        storage.save_state(&versioned).unwrap();

        let assets_in: i128 = 1000;
        let shares_burned = SorobanVaultContract::preview_withdraw(env.clone(), assets_in).unwrap();
        assert_eq!(shares_burned, 601);

        let shares_in: i128 = 800;
        let assets_out = SorobanVaultContract::preview_redeem(env.clone(), shares_in).unwrap();
        assert_eq!(assets_out, 1333);
    });
}

#[rstest]
fn soroban_contract_execute_withdraw_queue_empty_errors(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let user = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        let result = SorobanVaultContract::execute_withdraw(env.clone(), user);
        assert!(result.is_err());
    });
}

#[rstest]
fn soroban_contract_execute_withdraw_non_idle_errors(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let user = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        let mut state = VaultState::default();
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 0,
            plan: Vec::new(),
        });
        let mut storage = SorobanStorage::new(&env);
        let versioned = VersionedState::new(state);
        storage.save_state(&versioned).unwrap();
    });

    env.as_contract(&contract_id, || {
        let result = SorobanVaultContract::execute_withdraw(env.clone(), user);
        assert!(result.is_err());
    });
}

type TestVault = CuratorVault<MemoryStorage, TestPermissiveAuth, MockInterpreter>;

fn create_test_vault() -> TestVault {
    let mut vault = CuratorVault::new(
        test_config(),
        MemoryStorage::new(),
        TestPermissiveAuth,
        MockInterpreter::new(),
    );
    vault.load_state().unwrap();
    vault
}

#[fixture]
fn vault() -> TestVault {
    create_test_vault()
}

type RbacVault = CuratorVault<MemoryStorage, RbacAuth, MockInterpreter>;

fn create_rbac_vault() -> RbacVault {
    let mut rbac_config = RbacConfig::with_curator(curator_addr());
    rbac_config.add_role(guardian_addr(), Role::Guardian);
    rbac_config.add_role(sentinel_addr(), Role::Sentinel);
    rbac_config.add_role(allocator_addr(), Role::Allocator);

    let mut vault = CuratorVault::new(
        test_config(),
        MemoryStorage::new(),
        RbacAuth::new(rbac_config),
        MockInterpreter::new(),
    );
    vault.load_state().unwrap();
    vault
}

#[fixture]
fn rbac_vault() -> RbacVault {
    create_rbac_vault()
}

// Deposit Flow Tests

#[rstest]
fn test_deposit_flow_single(mut vault: TestVault) {
    let user = user_addr();
    let receiver = [11u8; 32];

    let result = vault.deposit(user, receiver, 1000, 0, 100).unwrap();

    assert_eq!(result.shares_minted, 1000);
    assert_eq!(result.total_shares, 1000);
    assert_eq!(result.total_assets, 1000);

    // Verify state
    assert_eq!(vault.state().unwrap().total_assets, 1000);
    assert_eq!(vault.state().unwrap().total_shares, 1000);
    assert_eq!(vault.state().unwrap().idle_assets, 1000);
    assert_eq!(vault.state().unwrap().external_assets, 0);
}

#[rstest]
fn test_deposit_flow_multiple(mut vault: TestVault) {
    let user = user_addr();
    let receiver = [11u8; 32];

    // First deposit
    vault.deposit(user, receiver, 1000, 0, 100).unwrap();

    // Second deposit - should maintain 1:1 ratio
    let result = vault.deposit(user, receiver, 500, 0, 200).unwrap();

    assert_eq!(result.shares_minted, 500);
    assert_eq!(result.total_shares, 1500);
    assert_eq!(result.total_assets, 1500);
}

#[rstest]
fn test_deposit_flow_with_slippage_protection(mut vault: TestVault) {
    let user = user_addr();
    let receiver = [11u8; 32];

    // First deposit to establish ratio
    vault.deposit(user, receiver, 1000, 0, 100).unwrap();

    // Second deposit with min_shares_out
    let result = vault.deposit(user, receiver, 500, 500, 200).unwrap();
    assert_eq!(result.shares_minted, 500);

    // Deposit that would violate slippage should fail
    let result = vault.deposit(user, receiver, 100, 200, 300);
    assert!(result.is_err());
}

#[rstest]
fn test_deposit_flow_zero_amount_fails(mut vault: TestVault) {
    let user = user_addr();
    let receiver = [11u8; 32];

    let result = vault.deposit(user, receiver, 0, 0, 100);
    assert!(result.is_err());
}

// Allocation Flow Tests

#[rstest]
fn test_allocation_flow_basic(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let allocator = allocator_addr();
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();
    assert_eq!(vault.state().unwrap().idle_assets, 10000);

    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 3000,
            }),
        )
        .unwrap();
    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 1,
                amount: 2000,
            }),
        )
        .unwrap();

    assert!(vault.state().unwrap().op_state.is_idle());
    assert_eq!(vault.state().unwrap().external_assets, 5000);
    assert_eq!(vault.state().unwrap().idle_assets, 5000);
}

#[rstest]
fn test_begin_allocating_decrements_idle_assets(mut vault: TestVault) {
    let allocator = allocator_addr();
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();
    let initial_idle = vault.state().unwrap().idle_assets;

    let alloc_total = 4000;
    let _op_id = vault
        .begin_allocating(allocator, vec![(0, alloc_total)], 1000)
        .unwrap();

    assert!(vault.state().unwrap().op_state.is_allocating());
    assert_eq!(
        vault.state().unwrap().idle_assets,
        initial_idle - alloc_total
    );
    assert_eq!(
        vault.state().unwrap().total_assets,
        vault.state().unwrap().idle_assets + vault.state().unwrap().external_assets
    );
}

#[rstest]
fn test_allocation_flow_wrong_op_id_fails(mut vault: TestVault) {
    let allocator = allocator_addr();
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    let op_id = vault
        .begin_allocating(allocator, vec![(0, 5000)], 1000)
        .unwrap();

    // Try to finish with wrong op_id
    let result = vault.finish_allocating(allocator, op_id + 999);
    assert!(result.is_err());
}

// Refresh Flow Tests

#[rstest]
fn test_refresh_flow_basic(mut vault: TestVault) {
    let allocator = allocator_addr();
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    let result = vault
        .refresh_markets(allocator, vec![0, 1, 2], 1000)
        .unwrap();

    assert!(vault.state().unwrap().op_state.is_idle());
    assert_eq!(result.markets_refreshed, 3);
    // No allocation was done, so markets hold 0 principal — external_assets stays 0.
    assert_eq!(result.new_external_assets, 0);
    assert_eq!(vault.state().unwrap().external_assets, 0);
}

// RBAC Tests

#[rstest]
fn test_rbac_user_can_deposit(mut rbac_vault: RbacVault) {
    let user = user_addr();

    // User should be able to deposit
    let result = rbac_vault.deposit(user, user, 1000, 0, 100);
    assert!(result.is_ok());
}

#[rstest]
fn test_rbac_user_cannot_allocate(mut rbac_vault: RbacVault) {
    let user = user_addr();

    // Setup
    rbac_vault
        .deposit(curator_addr(), curator_addr(), 10000, 0, 100)
        .unwrap();

    // User should not be able to begin allocation
    let result = rbac_vault.begin_allocating(user, vec![(0, 5000)], 1000);
    assert!(result.is_err());
}

#[rstest]
fn test_rbac_allocator_can_allocate(mut rbac_vault: RbacVault) {
    let allocator = allocator_addr();

    // Setup
    rbac_vault
        .deposit(curator_addr(), curator_addr(), 10000, 0, 100)
        .unwrap();

    // Allocator should be able to begin allocation
    let result = rbac_vault.begin_allocating(allocator, vec![(0, 5000)], 1000);
    assert!(result.is_ok());
}

#[rstest]
fn test_rbac_curator_can_do_everything(mut rbac_vault: RbacVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let curator = curator_addr();

    rbac_vault.deposit(curator, curator, 10000, 0, 100).unwrap();

    rbac_vault
        .allocate(
            curator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 5000,
            }),
        )
        .unwrap();

    rbac_vault.refresh_markets(curator, vec![0], 1000).unwrap();
}

#[rstest]
fn test_rbac_pause_by_sentinel(mut rbac_vault: RbacVault) {
    let sentinel = sentinel_addr();

    // Sentinel should be able to pause
    let result = rbac_vault.pause(sentinel, true);
    assert!(result.is_ok());
}

#[rstest]
fn test_rbac_user_cannot_pause(mut rbac_vault: RbacVault) {
    let user = user_addr();

    // User should not be able to pause
    let result = rbac_vault.pause(user, true);
    assert!(result.is_err());
}

#[rstest]
fn test_restrictions_blacklist_blocks_deposit(mut rbac_vault: RbacVault) {
    use templar_vault_kernel::Restrictions;

    let curator = curator_addr();
    let user = user_addr();

    rbac_vault
        .set_restrictions(curator, Some(Restrictions::Blacklist(vec![user])))
        .unwrap();

    let result = rbac_vault.deposit(user, user, 1000, 0, 100);
    assert!(result.is_err());
}

// State Persistence Tests

#[rstest]
fn test_state_persists_after_deposit(mut vault: TestVault) {
    let user = user_addr();

    vault.deposit(user, user, 1000, 0, 100).unwrap();

    // Verify storage was updated
    let stored = vault.storage.load_state().unwrap().unwrap();
    assert_eq!(stored.state.total_assets, 1000);
    assert_eq!(stored.state.total_shares, 1000);
}

#[rstest]
fn test_state_persists_after_allocation(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let allocator = allocator_addr();
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 5000,
            }),
        )
        .unwrap();

    let stored = vault.storage.load_state().unwrap().unwrap();
    assert_eq!(stored.state.external_assets, 5000);
    assert!(stored.state.op_state.is_idle());
}

// Effect Execution Tests

#[rstest]
fn test_deposit_emits_mint_effect(mut vault: TestVault) {
    let user = user_addr();
    let receiver = [11u8; 32];

    vault.deposit(user, receiver, 1000, 0, 100).unwrap();

    // Check that MintShares effect was recorded
    let effects = &vault.interpreter.effects;
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            templar_vault_kernel::effects::KernelEffect::MintShares { shares: 1000, .. }
        )),
        "expected mint effect for 1000 shares"
    );
}

// Full Flow Integration Tests

#[rstest]
fn test_full_flow_deposit_allocate_refresh(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();
    assert_eq!(vault.state().unwrap().total_assets, 10000);
    assert_eq!(vault.state().unwrap().idle_assets, 10000);

    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 5000,
            }),
        )
        .unwrap();
    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 1,
                amount: 3000,
            }),
        )
        .unwrap();

    assert_eq!(vault.state().unwrap().external_assets, 8000);
    assert_eq!(vault.state().unwrap().idle_assets, 2000);

    let result = vault.refresh_markets(allocator, vec![0, 1], 1000).unwrap();

    assert_eq!(result.new_external_assets, 8000);
    assert_eq!(vault.state().unwrap().external_assets, 8000);
}

// Withdraw Flow Tests

#[rstest]
fn test_withdraw_request_basic(mut vault: TestVault) {
    let user = user_addr();

    // First deposit some funds
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Request withdrawal
    let result = vault.request_withdraw(user, user, 1000, 0, 200).unwrap();

    assert!(result.shares_escrowed > 0);
    assert_eq!(result.shares_escrowed, 1000);
    let (id, pending) = vault
        .state()
        .unwrap()
        .withdraw_queue
        .head()
        .expect("pending withdrawal");
    assert_eq!(id, result.request_id);
    assert_eq!(pending.owner, user);
    assert_eq!(pending.receiver, user);
    assert_eq!(pending.escrow_shares, 1000);
}

#[rstest]
fn test_withdraw_request_calculates_assets_correctly(mut vault: TestVault) {
    let user = user_addr();

    // Deposit and establish share ratio
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Withdraw half the shares - should get half the assets
    let result = vault.request_withdraw(user, user, 5000, 4000, 200).unwrap();
    assert_eq!(result.shares_escrowed, 5000);
    let (_, pending) = vault
        .state()
        .unwrap()
        .withdraw_queue
        .head()
        .expect("pending withdrawal");
    assert_eq!(pending.expected_assets, 5000);
}

#[rstest]
fn test_withdraw_request_slippage_protection(mut vault: TestVault) {
    let user = user_addr();

    // Deposit
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Withdraw with unrealistic min_assets_out should fail
    let result = vault.request_withdraw(user, user, 1000, 2000, 200);
    assert!(result.is_err());
}

#[rstest]
fn test_withdraw_request_zero_shares_fails(mut vault: TestVault) {
    let user = user_addr();

    // Deposit first
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Try to withdraw zero shares
    let result = vault.request_withdraw(user, user, 0, 0, 200);
    assert!(result.is_err());
}

#[rstest]
fn test_withdraw_request_no_shares_fails(mut vault: TestVault) {
    let user = user_addr();

    // Try to withdraw without any deposits
    let result = vault.request_withdraw(user, user, 1000, 0, 100);
    assert!(result.is_err());
}

#[rstest]
fn test_execute_withdraw_requires_idle(mut vault: TestVault) {
    let user = user_addr();
    let allocator = allocator_addr();

    // Deposit
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Start allocation (vault not idle)
    vault
        .begin_allocating(allocator, vec![(0, 5000)], 1000)
        .unwrap();

    // Execute withdraw should fail when not idle
    let result = vault.execute_withdraw(user, 200);
    assert!(result.is_err());
}

#[rstest]
fn test_execute_withdraw_in_idle_state(mut vault: TestVault) {
    let user = user_addr();

    // Deposit
    vault.deposit(user, user, 10000, 0, 100).unwrap();
    vault.request_withdraw(user, user, 1000, 0, 0).unwrap();

    // Execute withdraw in idle state
    let result = vault.execute_withdraw(user, DEFAULT_COOLDOWN_NS + 1);
    assert!(result.is_ok());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
}

#[rstest]
fn test_execute_withdraw_respects_cooldown(mut vault: TestVault) {
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();
    vault.request_withdraw(user, user, 1000, 0, 0).unwrap();

    let early = vault.execute_withdraw(user, DEFAULT_COOLDOWN_NS - 1);
    assert!(early.is_err());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(!vault.state().unwrap().withdraw_queue.is_empty());

    let ok = vault.execute_withdraw(user, DEFAULT_COOLDOWN_NS + 1);
    assert!(ok.is_ok());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
}

#[rstest]
fn test_withdraw_flow_with_allocation(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 5000,
            }),
        )
        .unwrap();

    let result = vault.request_withdraw(user, user, 2000, 0, 300);
    assert!(result.is_ok());
}

// Full Flow Integration Tests - Deposit, Allocate, Refresh, Withdraw

#[rstest]
fn test_full_flow_deposit_allocate_refresh_withdraw(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();
    assert_eq!(vault.state().unwrap().total_assets, 10000);
    assert_eq!(vault.state().unwrap().total_shares, 10000);

    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 5000,
            }),
        )
        .unwrap();
    assert_eq!(vault.state().unwrap().external_assets, 5000);

    vault.refresh_markets(allocator, vec![0], 1000).unwrap();
    assert_eq!(vault.state().unwrap().external_assets, 5000);

    let result = vault.request_withdraw(user, user, 4000, 0, 0).unwrap();
    assert!(result.shares_escrowed > 0);

    let result = vault.execute_withdraw(user, DEFAULT_COOLDOWN_NS + 1);
    assert!(result.is_ok());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
}

#[rstest]
fn test_happy_path_like_near_sequence(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10_000, 0, 100).unwrap();

    vault.request_withdraw(user, user, 2_000, 0, 101).unwrap();
    vault
        .execute_withdraw(user, 101 + DEFAULT_COOLDOWN_NS + 1)
        .unwrap();

    assert!(vault.state().unwrap().withdraw_queue.is_empty());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert_eq!(vault.state().unwrap().total_assets, 8_000);
    assert_eq!(vault.state().unwrap().total_shares, 8_000);
    assert_eq!(vault.state().unwrap().idle_assets, 8_000);
    assert_eq!(vault.state().unwrap().external_assets, 0);

    vault.deposit(user, user, 4_000, 0, 200).unwrap();
    assert_eq!(vault.state().unwrap().total_assets, 12_000);
    assert_eq!(vault.state().unwrap().total_shares, 12_000);
    assert_eq!(vault.state().unwrap().idle_assets, 12_000);

    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 9_000,
            }),
        )
        .unwrap();

    assert_eq!(vault.state().unwrap().idle_assets, 3_000);
    assert_eq!(vault.state().unwrap().external_assets, 9_000);
    assert_eq!(vault.state().unwrap().total_assets, 12_000);

    vault.request_withdraw(user, user, 3_000, 0, 400).unwrap();
    vault
        .execute_withdraw(user, 400 + DEFAULT_COOLDOWN_NS + 1)
        .unwrap();

    assert!(vault.state().unwrap().withdraw_queue.is_empty());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert_eq!(vault.state().unwrap().idle_assets, 0);
    assert_eq!(vault.state().unwrap().external_assets, 9_000);
    assert_eq!(vault.state().unwrap().total_assets, 9_000);
    assert_eq!(vault.state().unwrap().total_shares, 9_000);
}

#[rstest]
fn test_withdraw_queue_orders_and_dequeues(mut vault: TestVault) {
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    let first = vault.request_withdraw(user, user, 1000, 0, 0).unwrap();
    let second = vault.request_withdraw(user, user, 2000, 0, 1).unwrap();

    let (head_id, pending) = vault
        .state()
        .unwrap()
        .withdraw_queue
        .head()
        .expect("pending withdrawal");
    assert_eq!(head_id, first.request_id);
    assert_eq!(pending.escrow_shares, 1000);

    vault
        .execute_withdraw(user, DEFAULT_COOLDOWN_NS + 1)
        .unwrap();

    let (next_id, next_pending) = vault
        .state()
        .unwrap()
        .withdraw_queue
        .head()
        .expect("second pending withdrawal");
    assert_eq!(next_id, second.request_id);
    assert_eq!(next_pending.escrow_shares, 2000);
}

// Edge Case Tests

#[rstest]
fn test_multiple_deposits_share_calculation(mut vault: TestVault) {
    let user1 = user_addr();
    let user2 = [20u8; 32];

    // First deposit - 1:1 ratio
    vault.deposit(user1, user1, 1000, 0, 100).unwrap();
    assert_eq!(vault.state().unwrap().total_shares, 1000);

    // Second deposit - should maintain ratio
    let result = vault.deposit(user2, user2, 2000, 0, 200).unwrap();
    assert_eq!(result.shares_minted, 2000);
    assert_eq!(vault.state().unwrap().total_shares, 3000);
    assert_eq!(vault.state().unwrap().total_assets, 3000);
}

#[rstest]
fn test_share_dilution_after_yield(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let user1 = user_addr();
    let user2 = [20u8; 32];
    let allocator = allocator_addr();

    vault.deposit(user1, user1, 1000, 0, 100).unwrap();

    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 500,
            }),
        )
        .unwrap();

    vault.refresh_markets(allocator, vec![0], 1000).unwrap();

    assert_eq!(vault.state().unwrap().external_assets, 500);
    let total_assets = vault.state().unwrap().total_assets;
    let total_shares = vault.state().unwrap().total_shares;

    let expected_shares = 1100u128 * total_shares / total_assets;
    let result = vault.deposit(user2, user2, 1100, 0, 300).unwrap();
    assert_eq!(result.shares_minted, expected_shares);
}

#[rstest]
fn test_allocation_multiple_markets(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 3000,
            }),
        )
        .unwrap();
    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 1,
                amount: 2000,
            }),
        )
        .unwrap();
    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 2,
                amount: 1000,
            }),
        )
        .unwrap();

    assert_eq!(vault.state().unwrap().external_assets, 6000);
    assert_eq!(vault.state().unwrap().idle_assets, 4000);
}

#[rstest]
fn test_refresh_multiple_markets(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 5000,
            }),
        )
        .unwrap();

    let result = vault
        .refresh_markets(allocator, vec![0, 1, 2], 1000)
        .unwrap();

    assert_eq!(result.new_external_assets, 5000);
    assert_eq!(vault.state().unwrap().external_assets, 5000);
}

// Concurrency / State Machine Tests

#[rstest]
fn test_cannot_allocate_while_allocating(mut vault: TestVault) {
    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Start first allocation
    vault
        .begin_allocating(allocator, vec![(0, 5000)], 1000)
        .unwrap();

    // Try to start second allocation - should fail
    let result = vault.begin_allocating(allocator, vec![(1, 3000)], 1000);
    assert!(result.is_err());
}

#[rstest]
fn test_cannot_refresh_while_allocating(mut vault: TestVault) {
    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Start allocation
    vault
        .begin_allocating(allocator, vec![(0, 5000)], 1000)
        .unwrap();

    // Try to start refresh - should fail
    let result = vault.begin_refreshing(allocator, vec![0, 1], 1000);
    assert!(result.is_err());
}

#[rstest]
fn test_cannot_allocate_while_refreshing(mut vault: TestVault) {
    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Start refresh
    vault.begin_refreshing(allocator, vec![0, 1], 1000).unwrap();

    // Try to start allocation - should fail
    let result = vault.begin_allocating(allocator, vec![(0, 5000)], 1000);
    assert!(result.is_err());
}

#[fixture]
fn dummy_ctx() -> EffectContext {
    EffectContext::new(0, [1u8; 32], [2u8; 32], [3u8; 32])
}

#[fixture]
fn mock_interpreter() -> MockInterpreter {
    MockInterpreter::new()
}

#[rstest]
fn test_deposit_effects_execute(mut mock_interpreter: MockInterpreter, dummy_ctx: EffectContext) {
    let effects = vec![
        KernelEffect::MintShares {
            owner: [9u8; 32],
            shares: 100,
        },
        KernelEffect::EmitEvent {
            event: templar_vault_kernel::effects::KernelEvent::DepositProcessed {
                owner: [8u8; 32],
                receiver: [9u8; 32],
                assets_in: 1000,
                shares_out: 100,
            },
        },
    ];

    let summary = mock_interpreter
        .execute_effects(&effects, &dummy_ctx)
        .unwrap();
    assert_eq!(summary.shares_minted, 100);
    assert_eq!(summary.events_emitted, 1);
    assert_eq!(mock_interpreter.effects.len(), 2);
}

#[rstest]
fn test_allocation_transition_flow_reaches_idle(
    mut mock_interpreter: MockInterpreter,
    dummy_ctx: EffectContext,
) {
    let op_id = 7u64;
    let plan = vec![(0u32, 100u128), (1u32, 200u128)];

    let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
    mock_interpreter
        .execute_effects(&result.effects, &dummy_ctx)
        .unwrap();
    let mut state = result.new_state;

    state = allocation_step_callback(state, true, 100, op_id)
        .unwrap()
        .new_state;
    state = allocation_step_callback(state, true, 200, op_id)
        .unwrap()
        .new_state;

    let result = complete_allocation(state, op_id, None).unwrap();
    mock_interpreter
        .execute_effects(&result.effects, &dummy_ctx)
        .unwrap();
    assert!(matches!(result.new_state, OpState::Idle));
}

#[rstest]
fn test_refresh_transition_flow_reaches_idle(
    mut mock_interpreter: MockInterpreter,
    dummy_ctx: EffectContext,
) {
    let op_id = 12u64;
    let plan = vec![0u32, 1u32, 2u32];

    let result = start_refresh(OpState::Idle, plan.clone(), op_id).unwrap();
    mock_interpreter
        .execute_effects(&result.effects, &dummy_ctx)
        .unwrap();
    let mut state = result.new_state;

    for _ in plan {
        state = refresh_step_callback(state, op_id).unwrap().new_state;
    }

    let result = complete_refresh(state, op_id).unwrap();
    mock_interpreter
        .execute_effects(&result.effects, &dummy_ctx)
        .unwrap();
    assert!(matches!(result.new_state, OpState::Idle));
}

#[rstest]
fn test_withdrawal_transition_flow_reaches_idle(
    mut mock_interpreter: MockInterpreter,
    dummy_ctx: EffectContext,
) {
    let op_id = 33u64;

    let request = WithdrawalRequest {
        op_id,
        amount: 150,
        receiver: [6u8; 32],
        owner: [5u8; 32],
        escrow_shares: 150,
    };

    let result = start_withdrawal(OpState::Idle, request).unwrap();
    mock_interpreter
        .execute_effects(&result.effects, &dummy_ctx)
        .unwrap();
    let state = result.new_state;

    let state = withdrawal_step_callback(state, op_id, 150)
        .unwrap()
        .new_state;
    let result = withdrawal_collected(state, op_id, 150).unwrap();
    mock_interpreter
        .execute_effects(&result.effects, &dummy_ctx)
        .unwrap();

    let escrow_address = dummy_ctx.vault_address;
    let result = payout_complete(result.new_state, true, op_id, escrow_address).unwrap();
    mock_interpreter
        .execute_effects(&result.effects, &dummy_ctx)
        .unwrap();
    assert!(matches!(result.new_state, OpState::Idle));
}

#[rstest]
fn test_refresh_state_roundtrip() {
    let state = OpState::Refreshing(RefreshingState {
        op_id: 9,
        index: 0,
        plan: vec![0, 1],
    });
    let result = refresh_step_callback(state, 9).unwrap();
    assert!(matches!(result.new_state, OpState::Refreshing(_)));
}
