//! Integration tests for the Soroban curator vault.
//!
//! These tests verify full flows: deposit -> allocate -> refresh -> withdraw.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use rstest::{fixture, rstest};
use soroban_sdk::{testutils::Address as _, Env};
use templar_curator_primitives::{RecoveryContext, RecoveryProgress};
use templar_soroban_runtime::{
    auth::PermissiveAuth,
    contract::{ContractConfig, CuratorVault, SorobanVaultContract},
    error::{ContractError, RuntimeError},
    market::{AttemptId, CrossChainMarketAdapter, MarketAdapter, MarketRef, SettlementReceipt},
    rbac::{RbacAuth, RbacConfig, Role},
    storage::{MemoryStorage, SorobanStorage, VersionedState},
    Storage, // Import the trait
};
use templar_vault_kernel::state::queue::DEFAULT_COOLDOWN_NS;
use templar_vault_kernel::{
    apply_action, Address, AllocatingState, FeesSpec, KernelAction, OpState, PayoutOutcome,
    PayoutState, VaultConfig, VaultState, WithdrawingState, MAX_PENDING, MIN_WITHDRAWAL_ASSETS,
};

mod common;
use common::MockInterpreter;

// Test Helpers

/// Mock market adapter that tracks calls.
#[derive(Clone, Debug, Default)]
struct TrackingMarketAdapter {
    pub supply_calls: Vec<(u32, u128)>,
    pub withdraw_calls: Vec<(u32, u128)>,
    pub total_assets_per_market: Vec<u128>,
    pub fail_on_market: Option<u32>,
}

impl TrackingMarketAdapter {
    fn new() -> Self {
        Self {
            supply_calls: Vec::new(),
            withdraw_calls: Vec::new(),
            total_assets_per_market: vec![1000, 2000, 3000], // default assets
            fail_on_market: None,
        }
    }
}

impl MarketAdapter for TrackingMarketAdapter {
    fn supply(&mut self, market: MarketRef, amount: u128) -> Result<(), RuntimeError> {
        if Some(market.market_id) == self.fail_on_market {
            return Err(RuntimeError::effect_failed("market supply failed"));
        }
        self.supply_calls.push((market.market_id, amount));
        Ok(())
    }

    fn withdraw(&mut self, market: MarketRef, amount: u128) -> Result<(), RuntimeError> {
        if Some(market.market_id) == self.fail_on_market {
            return Err(RuntimeError::effect_failed("market withdraw failed"));
        }
        self.withdraw_calls.push((market.market_id, amount));
        Ok(())
    }

    fn total_assets(&self, market: MarketRef) -> Result<u128, RuntimeError> {
        if Some(market.market_id) == self.fail_on_market {
            return Err(RuntimeError::effect_failed("market total_assets failed"));
        }
        let idx = market.market_id as usize;
        Ok(*self.total_assets_per_market.get(idx).unwrap_or(&0))
    }
}

/// Mock cross-chain adapter.
#[derive(Clone, Debug, Default)]
struct MockCrossChainAdapter {
    next_attempt: AttemptId,
    settled_external_assets: i128,
}

impl MockCrossChainAdapter {
    fn new() -> Self {
        Self {
            next_attempt: 1,
            settled_external_assets: 5000,
        }
    }
}

impl CrossChainMarketAdapter for MockCrossChainAdapter {
    fn submit_intent(&mut self, _plan_bytes: Vec<u8>) -> Result<AttemptId, RuntimeError> {
        let id = self.next_attempt;
        self.next_attempt += 1;
        Ok(id)
    }

    fn settle(
        &mut self,
        op_id: u64,
        attempt_id: AttemptId,
    ) -> Result<SettlementReceipt, RuntimeError> {
        Ok(SettlementReceipt {
            op_id,
            attempt_id,
            new_external_assets: self.settled_external_assets,
        })
    }

    fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
        Ok(self.settled_external_assets as u128)
    }
}

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

fn allocator_addr() -> Address {
    [3u8; 32]
}

fn user_addr() -> Address {
    [10u8; 32]
}

#[test]
fn soroban_contract_blend_config_roundtrip() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);
    let adapter = soroban_sdk::Address::generate(&env);
    let pool = soroban_sdk::Address::generate(&env);
    let factory = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator.clone(), asset, share).unwrap();
    });
    env.as_contract(&contract_id, || {
        SorobanVaultContract::set_blend_adapter(env.clone(), curator.clone(), adapter.clone())
            .unwrap();
    });
    env.as_contract(&contract_id, || {
        SorobanVaultContract::set_blend_pool(env.clone(), curator.clone(), pool.clone()).unwrap();
    });
    env.as_contract(&contract_id, || {
        SorobanVaultContract::set_blend_factory(env.clone(), curator, factory.clone()).unwrap();
    });
    env.as_contract(&contract_id, || {
        assert_eq!(
            SorobanVaultContract::blend_adapter(env.clone()).unwrap(),
            adapter
        );
        assert_eq!(SorobanVaultContract::blend_pool(env.clone()).unwrap(), pool);
        assert_eq!(
            SorobanVaultContract::blend_factory(env.clone()).unwrap(),
            factory
        );
    });
}

#[test]
fn soroban_contract_blend_config_rejects_account_addresses() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);
    let account = soroban_sdk::Address::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    );

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator.clone(), asset, share).unwrap();
    });

    env.as_contract(&contract_id, || {
        let err =
            SorobanVaultContract::set_blend_adapter(env.clone(), curator.clone(), account.clone())
                .unwrap_err();
        assert_eq!(err, ContractError::InvalidInput);
    });

    env.as_contract(&contract_id, || {
        let err =
            SorobanVaultContract::set_blend_pool(env.clone(), curator.clone(), account.clone())
                .unwrap_err();
        assert_eq!(err, ContractError::InvalidInput);
    });

    env.as_contract(&contract_id, || {
        let err =
            SorobanVaultContract::set_blend_factory(env.clone(), curator, account).unwrap_err();
        assert_eq!(err, ContractError::InvalidInput);
    });
}

#[test]
fn soroban_contract_vault_snapshot_matches_fields() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator, asset, share).unwrap();
    });
    env.as_contract(&contract_id, || {
        let snapshot = SorobanVaultContract::vault_snapshot(env.clone());
        assert_eq!(
            snapshot.total_shares,
            SorobanVaultContract::total_shares(env.clone())
        );
        assert_eq!(
            snapshot.idle_assets,
            SorobanVaultContract::idle_assets(env.clone())
        );
        assert_eq!(
            snapshot.external_assets,
            SorobanVaultContract::external_assets(env.clone())
        );
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

#[test]
fn soroban_contract_preview_deposit_matches_kernel() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator, asset, share).unwrap();
    });

    let assets_in = 500u128;

    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        let empty_state = VaultState::default();
        let versioned = VersionedState::new(empty_state.clone());
        storage.save_state(&versioned).unwrap();

        let preview = SorobanVaultContract::preview_deposit(env.clone(), assets_in as i128);
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

        let preview = SorobanVaultContract::preview_deposit(env.clone(), assets_in as i128);
        let minted = mint_shares_from_deposit(state, assets_in);
        assert_eq!(preview as u128, minted);
    });
}

#[test]
fn soroban_contract_preview_withdraw_matches_kernel() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator, asset, share).unwrap();
    });

    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        let mut state = VaultState::default();
        state.total_assets = 20_000;
        state.total_shares = 12_000;
        state.idle_assets = 20_000;
        let versioned = VersionedState::new(state.clone());
        storage.save_state(&versioned).unwrap();

        // ERC-4626: preview_withdraw(assets) returns shares to burn (ceil)
        let assets_in: i128 = 1000;
        let shares_burned = SorobanVaultContract::preview_withdraw(env.clone(), assets_in);
        // Effective totals with virtual +1: (12001, 20001)
        // ceil(1000 * 12001 / 20001) = ceil(12001000/20001) = ceil(600.02) = 601
        assert_eq!(shares_burned, 601);

        // ERC-4626: preview_redeem(shares) returns assets received (floor)
        let shares_in: i128 = 800;
        let assets_out = SorobanVaultContract::preview_redeem(env.clone(), shares_in);
        // floor(800 * 20001 / 12001) = floor(16000800/12001) = floor(1333.22) = 1333
        assert_eq!(assets_out, 1333);
    });
}

#[test]
fn soroban_contract_execute_withdraw_queue_empty_errors() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);
    let user = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator, asset, share).unwrap();
    });

    env.as_contract(&contract_id, || {
        let result = SorobanVaultContract::execute_withdraw(env.clone(), user);
        assert!(result.is_err());
    });
}

#[test]
fn soroban_contract_execute_withdraw_non_idle_errors() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);
    let user = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator, asset, share).unwrap();
    });

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

type TestVault = CuratorVault<
    MemoryStorage,
    PermissiveAuth,
    MockInterpreter,
    TrackingMarketAdapter,
    MockCrossChainAdapter,
>;

fn create_test_vault() -> TestVault {
    let mut vault = CuratorVault::new(
        test_config(),
        MemoryStorage::new(),
        PermissiveAuth,
        MockInterpreter::new(),
        TrackingMarketAdapter::new(),
        MockCrossChainAdapter::new(),
    );
    vault.load_state().unwrap();
    vault
}

#[fixture]
fn vault() -> TestVault {
    create_test_vault()
}

type RbacVault = CuratorVault<
    MemoryStorage,
    RbacAuth,
    MockInterpreter,
    TrackingMarketAdapter,
    MockCrossChainAdapter,
>;

fn create_rbac_vault() -> RbacVault {
    let mut rbac_config = RbacConfig::with_curator(curator_addr());
    rbac_config.add_role(guardian_addr(), Role::Guardian);
    rbac_config.add_role(allocator_addr(), Role::Allocator);

    let mut vault = CuratorVault::new(
        test_config(),
        MemoryStorage::new(),
        RbacAuth::new(rbac_config),
        MockInterpreter::new(),
        TrackingMarketAdapter::new(),
        MockCrossChainAdapter::new(),
    );
    vault.load_state().unwrap();
    vault
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
    let allocator = allocator_addr();
    let user = user_addr();

    // Setup: deposit some funds
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Begin allocation
    let op_id = vault
        .begin_allocating(allocator, vec![(0, 3000), (1, 2000)], 1000)
        .unwrap();

    assert!(vault.state().unwrap().op_state.is_allocating());
    assert_eq!(op_id, 0);

    // Sync external assets (simulating market supply completion)
    vault
        .sync_external_assets(allocator, 5000, op_id, 1000)
        .unwrap();

    assert_eq!(vault.state().unwrap().external_assets, 5000);

    // Finish allocation
    let result = vault.finish_allocating(allocator, op_id).unwrap();

    assert_eq!(result.op_id, op_id);
    assert!(vault.state().unwrap().op_state.is_idle());
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
fn test_allocation_flow_abort(mut vault: TestVault) {
    let allocator = allocator_addr();
    let user = user_addr();

    // Setup: deposit some funds
    vault.deposit(user, user, 10000, 0, 100).unwrap();
    let initial_idle = vault.state().unwrap().idle_assets;

    // Begin allocation
    let op_id = vault
        .begin_allocating(allocator, vec![(0, 5000)], 1000)
        .unwrap();

    let restore_idle = vault
        .state()
        .unwrap()
        .op_state
        .as_allocating()
        .expect("allocating")
        .remaining;

    // Abort (restoring remaining to idle)
    vault
        .abort_allocating(allocator, op_id, restore_idle)
        .unwrap();

    assert!(vault.state().unwrap().op_state.is_idle());
    // After abort, idle_assets should be restored to pre-allocation value.
    // begin_allocating decremented idle by 5000, abort restores it.
    assert_eq!(vault.state().unwrap().idle_assets, initial_idle);
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

    // Setup
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Begin refresh
    let op_id = vault
        .begin_refreshing(allocator, vec![0, 1, 2], 1000)
        .unwrap();

    assert!(vault.state().unwrap().op_state.is_refreshing());

    // Sync external assets (simulating market read completion)
    vault
        .sync_external_assets(allocator, 6000, op_id, 1000)
        .unwrap();

    // Finish refresh
    let result = vault.finish_refreshing(allocator, op_id).unwrap();

    assert_eq!(result.op_id, op_id);
    assert!(vault.state().unwrap().op_state.is_idle());
    assert_eq!(vault.state().unwrap().external_assets, 6000);
}

#[rstest]
fn test_refresh_flow_abort(mut vault: TestVault) {
    let allocator = allocator_addr();
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    let op_id = vault.begin_refreshing(allocator, vec![0, 1], 1000).unwrap();

    // Abort refresh
    vault.abort_refreshing(allocator, op_id).unwrap();

    assert!(vault.state().unwrap().op_state.is_idle());
}

#[rstest]
fn test_abort_withdrawing_clears_queue(mut vault: TestVault) {
    let allocator = allocator_addr();
    let owner = user_addr();
    let receiver = user_addr();
    let expected_assets = 1_000;
    let escrow_shares = 500;

    let op_id = {
        let state = vault.state_mut().unwrap();
        state.total_shares = escrow_shares;
        state
            .withdraw_queue
            .enqueue(
                owner,
                receiver,
                escrow_shares,
                expected_assets,
                0,
                MAX_PENDING as u32,
            )
            .unwrap();

        let op_id = state.allocate_op_id();
        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id,
            index: 0,
            remaining: expected_assets,
            collected: 0,
            receiver,
            owner,
            escrow_shares,
        });
        op_id
    };

    vault
        .abort_withdrawing(allocator, op_id, escrow_shares)
        .unwrap();

    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
    assert!(!vault.interpreter.effects.is_empty());
}

#[rstest]
fn test_settle_payout_success_burns_and_dequeues(mut vault: TestVault) {
    let allocator = allocator_addr();
    let owner = user_addr();
    let receiver = user_addr();
    let escrow_shares = 500;
    let burn_shares = 300;
    let refund_shares = 200;
    let amount = 1_000;

    let op_id = {
        let state = vault.state_mut().unwrap();
        state.total_shares = escrow_shares;
        state
            .withdraw_queue
            .enqueue(
                owner,
                receiver,
                escrow_shares,
                amount,
                0,
                MAX_PENDING as u32,
            )
            .unwrap();
        let op_id = state.allocate_op_id();
        state.op_state = OpState::Payout(PayoutState {
            op_id,
            receiver,
            amount,
            owner,
            escrow_shares,
            burn_shares,
        });
        op_id
    };

    vault
        .settle_payout(
            allocator,
            op_id,
            PayoutOutcome::Success {
                burn_shares,
                refund_shares,
            },
        )
        .unwrap();

    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
    assert_eq!(
        vault.state().unwrap().total_shares,
        escrow_shares.saturating_sub(burn_shares)
    );
}

#[rstest]
fn test_recover_payout_failure_restores_idle(mut vault: TestVault) {
    let allocator = allocator_addr();
    let owner = user_addr();
    let receiver = user_addr();
    let escrow_shares = 500;
    let amount = 1_000;

    let _op_id = {
        let state = vault.state_mut().unwrap();
        state.total_shares = escrow_shares;
        state.idle_assets = 0;
        state.total_assets = 0;
        state
            .withdraw_queue
            .enqueue(
                owner,
                receiver,
                escrow_shares,
                amount,
                0,
                MAX_PENDING as u32,
            )
            .unwrap();
        let op_id = state.allocate_op_id();
        state.op_state = OpState::Payout(PayoutState {
            op_id,
            receiver,
            amount,
            owner,
            escrow_shares,
            burn_shares: 0,
        });
        op_id
    };

    let context = RecoveryContext::forced(0);
    let progress = RecoveryProgress::new(0);
    let summary = vault.recover(allocator, context, progress).unwrap();

    assert!(summary.is_some());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
    assert_eq!(vault.state().unwrap().idle_assets, amount);
    assert_eq!(vault.state().unwrap().total_assets, amount);
}

// RBAC Tests

#[test]
fn test_rbac_user_can_deposit() {
    let mut vault = create_rbac_vault();
    let user = user_addr();

    // User should be able to deposit
    let result = vault.deposit(user, user, 1000, 0, 100);
    assert!(result.is_ok());
}

#[test]
fn test_rbac_user_cannot_allocate() {
    let mut vault = create_rbac_vault();
    let user = user_addr();

    // Setup
    vault
        .deposit(curator_addr(), curator_addr(), 10000, 0, 100)
        .unwrap();

    // User should not be able to begin allocation
    let result = vault.begin_allocating(user, vec![(0, 5000)], 1000);
    assert!(result.is_err());
}

#[test]
fn test_rbac_allocator_can_allocate() {
    let mut vault = create_rbac_vault();
    let allocator = allocator_addr();

    // Setup
    vault
        .deposit(curator_addr(), curator_addr(), 10000, 0, 100)
        .unwrap();

    // Allocator should be able to begin allocation
    let result = vault.begin_allocating(allocator, vec![(0, 5000)], 1000);
    assert!(result.is_ok());
}

#[test]
fn test_rbac_curator_can_do_everything() {
    let mut vault = create_rbac_vault();
    let curator = curator_addr();

    // Deposit
    vault.deposit(curator, curator, 10000, 0, 100).unwrap();

    // Begin allocation (curator has all privileges)
    let op_id = vault
        .begin_allocating(curator, vec![(0, 5000)], 1000)
        .unwrap();

    // Sync external assets
    vault
        .sync_external_assets(curator, 5000, op_id, 1000)
        .unwrap();

    // Finish allocation
    vault.finish_allocating(curator, op_id).unwrap();

    // Begin refresh
    let op_id = vault.begin_refreshing(curator, vec![0], 1000).unwrap();
    vault.finish_refreshing(curator, op_id).unwrap();
}

#[test]
fn test_rbac_pause_by_guardian() {
    let mut vault = create_rbac_vault();
    let guardian = guardian_addr();

    // Guardian should be able to pause
    let result = vault.pause(guardian, true);
    assert!(result.is_ok());
}

#[test]
fn test_rbac_user_cannot_pause() {
    let mut vault = create_rbac_vault();
    let user = user_addr();

    // User should not be able to pause
    let result = vault.pause(user, true);
    assert!(result.is_err());
}

#[test]
fn test_restrictions_blacklist_blocks_deposit() {
    use templar_vault_kernel::Restrictions;

    let mut vault = create_rbac_vault();
    let curator = curator_addr();
    let user = user_addr();

    vault
        .set_restrictions(curator, Some(Restrictions::Blacklist(vec![user])))
        .unwrap();

    let result = vault.deposit(user, user, 1000, 0, 100);
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
    let allocator = allocator_addr();
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    let op_id = vault
        .begin_allocating(allocator, vec![(0, 5000)], 1000)
        .unwrap();
    vault
        .sync_external_assets(allocator, 5000, op_id, 1000)
        .unwrap();
    vault.finish_allocating(allocator, op_id).unwrap();

    // Verify storage was updated
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
    let user = user_addr();
    let allocator = allocator_addr();

    // 1. Deposit
    vault.deposit(user, user, 10000, 0, 100).unwrap();
    assert_eq!(vault.state().unwrap().total_assets, 10000);
    assert_eq!(vault.state().unwrap().idle_assets, 10000);

    // 2. Allocate to markets
    let op_id = vault
        .begin_allocating(allocator, vec![(0, 5000), (1, 3000)], 1000)
        .unwrap();
    vault
        .sync_external_assets(allocator, 8000, op_id, 1000)
        .unwrap();
    vault.finish_allocating(allocator, op_id).unwrap();

    assert_eq!(vault.state().unwrap().external_assets, 8000);

    // 3. Refresh markets
    let op_id = vault.begin_refreshing(allocator, vec![0, 1], 1000).unwrap();
    // Update adapter to reflect market growth (5000→5625, 3000→3375 = 9000 total)
    vault.market.total_assets_per_market = vec![5625, 3375, 0];
    vault
        .sync_external_assets(allocator, 9000, op_id, 1000)
        .unwrap(); // markets grew
    vault.finish_refreshing(allocator, op_id).unwrap();

    // Total assets should now be 10000 + 1000 (growth from 8000 to 9000)
    assert_eq!(vault.state().unwrap().external_assets, 9000);
    // Total assets = idle (10000 - 8000 = 2000 if we tracked correctly) + external (9000)
    // In our implementation, total_assets is adjusted based on external_assets changes
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
    let user = user_addr();
    let allocator = allocator_addr();

    // 1. Deposit
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // 2. Allocate some to markets
    let op_id = vault
        .begin_allocating(allocator, vec![(0, 5000)], 1000)
        .unwrap();
    vault
        .sync_external_assets(allocator, 5000, op_id, 1000)
        .unwrap();
    vault.finish_allocating(allocator, op_id).unwrap();

    // 3. Now request withdrawal (from idle state)
    let result = vault.request_withdraw(user, user, 2000, 0, 300);
    assert!(result.is_ok());
}

// Full Flow Integration Tests - Deposit, Allocate, Refresh, Withdraw

#[rstest]
fn test_full_flow_deposit_allocate_refresh_withdraw(mut vault: TestVault) {
    let user = user_addr();
    let allocator = allocator_addr();

    // 1. Deposit
    vault.deposit(user, user, 10000, 0, 100).unwrap();
    assert_eq!(vault.state().unwrap().total_assets, 10000);
    assert_eq!(vault.state().unwrap().total_shares, 10000);

    // 2. Allocate to markets
    let op_id = vault
        .begin_allocating(allocator, vec![(0, 5000)], 1000)
        .unwrap();
    vault
        .sync_external_assets(allocator, 5000, op_id, 1000)
        .unwrap();
    vault.finish_allocating(allocator, op_id).unwrap();
    assert_eq!(vault.state().unwrap().external_assets, 5000);

    // 3. Refresh - markets grew
    let op_id = vault.begin_refreshing(allocator, vec![0], 1000).unwrap();
    // Update adapter to reflect 20% market growth
    vault.market.total_assets_per_market[0] = 6000;
    vault
        .sync_external_assets(allocator, 6000, op_id, 1000)
        .unwrap(); // 20% growth
    vault.finish_refreshing(allocator, op_id).unwrap();
    assert_eq!(vault.state().unwrap().external_assets, 6000);

    // 4. Request withdrawal — use fewer shares so the payout fits in idle.
    // After yield growth: total_assets=11000, total_shares=10000, idle=5000.
    // 4000 shares × (11000/10000) = 4400 assets, within idle(5000).
    let result = vault.request_withdraw(user, user, 4000, 0, 0).unwrap();
    assert!(result.shares_escrowed > 0);

    // 5. Execute withdrawal (in idle state)
    let result = vault.execute_withdraw(user, DEFAULT_COOLDOWN_NS + 1);
    assert!(result.is_ok());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
}

#[rstest]
fn test_happy_path_like_near_sequence(mut vault: TestVault) {
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

    let alloc_op = vault
        .begin_allocating(allocator, vec![(0, 9_000)], 300)
        .unwrap();
    vault
        .sync_external_assets(allocator, 9_000, alloc_op, 301)
        .unwrap();
    vault.finish_allocating(allocator, alloc_op).unwrap();

    assert_eq!(vault.state().unwrap().idle_assets, 3_000);
    assert_eq!(vault.state().unwrap().external_assets, 9_000);
    assert_eq!(vault.state().unwrap().total_assets, 12_000);

    vault.request_withdraw(user, user, 4_000, 0, 400).unwrap();
    vault
        .execute_withdraw(user, 400 + DEFAULT_COOLDOWN_NS + 1)
        .unwrap();

    let withdraw_op = match &vault.state().unwrap().op_state {
        OpState::Withdrawing(state) => state.op_id,
        _ => panic!("expected withdrawing state"),
    };

    {
        let state = vault.state_mut().unwrap();
        state.idle_assets += 1_000;
        state.external_assets -= 1_000;
        state.total_assets = state.idle_assets + state.external_assets;
    }

    vault
        .sync_external_assets(allocator, 8_000, withdraw_op, 500)
        .unwrap();

    vault
        .execute_withdraw(user, 400 + DEFAULT_COOLDOWN_NS + 2)
        .unwrap();

    assert!(vault.state().unwrap().withdraw_queue.is_empty());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert_eq!(vault.state().unwrap().total_assets, 8_000);
    assert_eq!(vault.state().unwrap().total_shares, 8_000);
    assert_eq!(vault.state().unwrap().idle_assets, 0);
    assert_eq!(vault.state().unwrap().external_assets, 8_000);
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
    let user1 = user_addr();
    let user2 = [20u8; 32];
    let allocator = allocator_addr();

    // User1 deposits
    vault.deposit(user1, user1, 1000, 0, 100).unwrap();

    // Allocate half to external (keep some idle)
    let op_id = vault
        .begin_allocating(allocator, vec![(0, 500)], 1000)
        .unwrap();
    vault
        .sync_external_assets(allocator, 500, op_id, 1000)
        .unwrap();
    vault.finish_allocating(allocator, op_id).unwrap();

    // Market grows - 20% yield (500 -> 600)
    let op_id = vault.begin_refreshing(allocator, vec![0], 1000).unwrap();
    // Update adapter to reflect 20% yield on market 0
    vault.market.total_assets_per_market[0] = 600;
    vault
        .sync_external_assets(allocator, 600, op_id, 1000)
        .unwrap();
    vault.finish_refreshing(allocator, op_id).unwrap();

    // After refresh: external_assets = 600, total_assets is adjusted by the yield
    assert_eq!(vault.state().unwrap().external_assets, 600);
    let total_assets = vault.state().unwrap().total_assets;
    let total_shares = vault.state().unwrap().total_shares;

    // User2 deposits 1100
    // shares = amount * total_shares / total_assets
    let expected_shares = 1100u128 * total_shares / total_assets;
    let result = vault.deposit(user2, user2, 1100, 0, 300).unwrap();
    assert_eq!(result.shares_minted, expected_shares);
}

#[rstest]
fn test_allocation_multiple_markets(mut vault: TestVault) {
    let user = user_addr();
    let allocator = allocator_addr();

    // Deposit
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Allocate to multiple markets
    let op_id = vault
        .begin_allocating(allocator, vec![(0, 3000), (1, 2000), (2, 1000)], 1000)
        .unwrap();

    // Sync total external
    vault
        .sync_external_assets(allocator, 6000, op_id, 1000)
        .unwrap();
    vault.finish_allocating(allocator, op_id).unwrap();

    assert_eq!(vault.state().unwrap().external_assets, 6000);
}

#[rstest]
fn test_refresh_multiple_markets(mut vault: TestVault) {
    let user = user_addr();
    let allocator = allocator_addr();

    // Setup
    vault.deposit(user, user, 10000, 0, 100).unwrap();
    let op_id = vault
        .begin_allocating(allocator, vec![(0, 5000)], 1000)
        .unwrap();
    vault
        .sync_external_assets(allocator, 5000, op_id, 1000)
        .unwrap();
    vault.finish_allocating(allocator, op_id).unwrap();

    // Refresh multiple markets
    let op_id = vault
        .begin_refreshing(allocator, vec![0, 1, 2], 1000)
        .unwrap();
    vault
        .sync_external_assets(allocator, 6000, op_id, 1000)
        .unwrap();
    vault.finish_refreshing(allocator, op_id).unwrap();

    assert_eq!(vault.state().unwrap().external_assets, 6000);
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
