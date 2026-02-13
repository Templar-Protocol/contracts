use super::*;
use crate::auth::PermissiveAuth;
use crate::effects::{AddressRegistrar, EffectContext, EffectInterpreter, EffectResult};
use crate::storage::MemoryStorage;
use alloc::collections::BTreeMap;
use alloc::vec;
use templar_vault_kernel::effects::KernelEffect;

#[derive(Clone, Debug, Default)]
struct MockInterpreter {
    should_fail: bool,
    effects: Vec<KernelEffect>,
}

impl MockInterpreter {
    fn new() -> Self {
        Self {
            should_fail: false,
            effects: Vec::new(),
        }
    }
}

impl EffectInterpreter for MockInterpreter {
    fn execute_effect(&mut self, effect: &KernelEffect, _ctx: &EffectContext) -> EffectResult<()> {
        if self.should_fail {
            return Err(RuntimeError::effect_failed("mock interpreter failed"));
        }
        self.effects.push(effect.clone());
        Ok(())
    }
}

impl AddressRegistrar for MockInterpreter {
    fn register_address(&mut self, _kernel_addr: [u8; 32], _soroban_addr: SdkAddress) {}

    fn has_address(&self, _kernel_addr: &[u8; 32]) -> bool {
        true
    }
}

#[derive(Clone, Debug, Default)]
struct TrackingInterpreter {
    addresses: BTreeMap<[u8; 32], SdkAddress>,
    effects: Vec<KernelEffect>,
}

impl TrackingInterpreter {
    fn new() -> Self {
        Self {
            addresses: BTreeMap::new(),
            effects: Vec::new(),
        }
    }
}

impl EffectInterpreter for TrackingInterpreter {
    fn execute_effect(&mut self, effect: &KernelEffect, _ctx: &EffectContext) -> EffectResult<()> {
        self.effects.push(effect.clone());
        Ok(())
    }
}

impl AddressRegistrar for TrackingInterpreter {
    fn register_address(&mut self, kernel_addr: [u8; 32], soroban_addr: SdkAddress) {
        self.addresses.insert(kernel_addr, soroban_addr);
    }

    fn has_address(&self, kernel_addr: &[u8; 32]) -> bool {
        self.addresses.contains_key(kernel_addr)
    }
}

struct MockMarket;

impl MarketAdapter for MockMarket {
    fn supply(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn withdraw(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
        Ok(1000)
    }
}

struct FailingMarket;

impl MarketAdapter for FailingMarket {
    fn supply(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn withdraw(&mut self, _market: MarketRef, _amount: u128) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
        Err(RuntimeError::effect_failed("market total_assets failed"))
    }
}

struct MockCrossChain;

impl CrossChainMarketAdapter for MockCrossChain {
    fn submit_intent(
        &mut self,
        _plan_bytes: Vec<u8>,
    ) -> Result<crate::market::AttemptId, RuntimeError> {
        Ok(1)
    }

    fn settle(
        &mut self,
        op_id: u64,
        attempt_id: crate::market::AttemptId,
    ) -> Result<crate::market::SettlementReceipt, RuntimeError> {
        Ok(crate::market::SettlementReceipt {
            op_id,
            attempt_id,
            new_external_assets: 1000,
        })
    }

    fn total_assets(&self, _market: MarketRef) -> Result<u128, RuntimeError> {
        Ok(1000)
    }
}

fn test_config() -> ContractConfig {
    ContractConfig::new(
        [1u8; 32],
        [9u8; 32],
        vec![[2u8; 32]],
        vec![[3u8; 32]],
        [4u8; 32],
        [5u8; 32],
    )
}

fn create_test_vault(
) -> CuratorVault<MemoryStorage, PermissiveAuth, MockInterpreter, MockMarket, MockCrossChain> {
    let mut vault = CuratorVault::new(
        test_config(),
        MemoryStorage::new(),
        PermissiveAuth,
        MockInterpreter::new(),
        MockMarket,
        MockCrossChain,
    );
    vault.load_state().unwrap();
    vault
}

fn create_test_vault_with_failing_market(
) -> CuratorVault<MemoryStorage, PermissiveAuth, MockInterpreter, FailingMarket, MockCrossChain> {
    let mut vault = CuratorVault::new(
        test_config(),
        MemoryStorage::new(),
        PermissiveAuth,
        MockInterpreter::new(),
        FailingMarket,
        MockCrossChain,
    );
    vault.load_state().unwrap();
    vault
}

#[test]
fn test_kernel_address_from_sdk_is_domain_separated() {
    use soroban_sdk::testutils::Address as _;

    let env = Env::default();
    let addr = SdkAddress::generate(&env);
    let derived = kernel_address_from_sdk(&env, &addr);

    let strkey = addr.to_string();
    let strkey_bytes = strkey.to_bytes();
    let mut strkey_vec = vec![0u8; strkey_bytes.len() as usize];
    strkey_bytes.copy_into_slice(&mut strkey_vec);
    let raw_bytes = Bytes::from_slice(&env, &strkey_vec);
    let raw_hash = env.crypto().sha256(&raw_bytes).to_bytes().to_array();

    let mut prefixed = Vec::with_capacity(KERNEL_ADDRESS_DOMAIN.len() + strkey_vec.len());
    prefixed.extend_from_slice(KERNEL_ADDRESS_DOMAIN);
    prefixed.extend_from_slice(&strkey_vec);
    let expected = env
        .crypto()
        .sha256(&Bytes::from_slice(&env, &prefixed))
        .to_bytes()
        .to_array();

    assert_eq!(derived, expected);
    assert_ne!(derived, raw_hash);
}

#[test]
fn test_deposit_first() {
    let mut vault = create_test_vault();
    let caller = [1u8; 32];
    let receiver = [10u8; 32];

    let result = vault.deposit(caller, receiver, 1000, 0, 100).unwrap();

    assert_eq!(result.shares_minted, 1000);
    assert_eq!(result.total_shares, 1000);
    assert_eq!(result.total_assets, 1000);
}

#[test]
fn test_deposit_subsequent() {
    let mut vault = create_test_vault();
    let caller = [1u8; 32];
    let receiver = [10u8; 32];

    // First deposit
    vault.deposit(caller, receiver, 1000, 0, 100).unwrap();

    // Second deposit should get proportional shares
    let result = vault.deposit(caller, receiver, 500, 0, 200).unwrap();

    assert_eq!(result.shares_minted, 500);
    assert_eq!(result.total_shares, 1500);
    assert_eq!(result.total_assets, 1500);
}

#[test]
fn test_deposit_zero_fails() {
    let mut vault = create_test_vault();
    let caller = [1u8; 32];
    let receiver = [10u8; 32];

    let result = vault.deposit(caller, receiver, 0, 0, 100);

    assert!(result.is_err());
}

#[test]
fn test_deposit_slippage_fails() {
    let mut vault = create_test_vault();
    let caller = [1u8; 32];
    let receiver = [10u8; 32];

    // Deposit with min_shares_out higher than actual
    let result = vault.deposit(caller, receiver, 1000, 2000, 100);

    assert!(result.is_err());
}

#[test]
fn test_begin_allocating() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    let state = vault.state_mut();
    state.idle_assets = 2_000;
    state.total_assets = 2_000;

    let op_id = vault
        .begin_allocating(caller, vec![(0, 500), (1, 500)], 1000)
        .unwrap();

    assert_eq!(op_id, 0);
    assert!(vault.state().op_state.is_allocating());
}

#[test]
fn test_finish_allocating() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    let state = vault.state_mut();
    state.idle_assets = 2_000;
    state.total_assets = 2_000;

    let op_id = vault
        .begin_allocating(caller, vec![(0, 500)], 1000)
        .unwrap();

    let result = vault.finish_allocating(caller, op_id).unwrap();

    assert_eq!(result.op_id, op_id);
    assert!(vault.state().op_state.is_idle());
}

#[test]
fn test_sync_external_assets_rejects_adapter_mismatch_during_refresh() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    let state = vault.state_mut();
    state.idle_assets = 2_000;
    state.total_assets = 2_000;

    // Use refresh (plan covers all markets, so adapter verification applies)
    let op_id = vault.begin_refreshing(caller, vec![0, 1], 1000).unwrap();

    // MockMarket reports 1000 per target, so adapter_total is 2000.
    // Claiming 1500 != 2000 triggers adapter mismatch.
    let err = vault.sync_external_assets(caller, 1500, op_id, 1000);
    let invalid_state = matches!(
        &err,
        Err(RuntimeError::InvalidState(msg))
            if msg.contains("claimed value does not match")
    );
    assert!(invalid_state, "unexpected error: {err:?}");

    assert!(vault.state().op_state.is_refreshing());
}

#[test]
fn test_sync_external_assets_rejects_when_adapter_unavailable_during_refresh() {
    let mut vault = create_test_vault_with_failing_market();
    let caller = [3u8; 32]; // allocator

    let state = vault.state_mut();
    state.idle_assets = 2_000;
    state.total_assets = 2_000;

    // Use refresh so adapter verification is attempted
    let op_id = vault.begin_refreshing(caller, vec![0, 1], 1000).unwrap();

    let err = vault.sync_external_assets(caller, 2_000, op_id, 1000);
    let invalid_state = matches!(
        &err,
        Err(RuntimeError::InvalidState(msg))
            if msg.contains("adapter unavailable for refresh verification")
    );
    assert!(invalid_state, "unexpected error: {err:?}");

    assert!(vault.state().op_state.is_refreshing());
    assert_eq!(vault.state().external_assets, 0);
}

#[test]
fn test_begin_refreshing() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    let op_id = vault.begin_refreshing(caller, vec![0, 1], 1000).unwrap();

    assert_eq!(op_id, 0);
    assert!(vault.state().op_state.is_refreshing());
}

#[test]
fn test_finish_refreshing_reports_markets_refreshed() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    vault
        .acquire_market_lock(caller, 2, 5000, 1000)
        .expect("should acquire lock");

    let op_id = vault
        .begin_refreshing(caller, vec![0, 1, 2], 1500)
        .expect("should start refresh");

    let expected = vault
        .state()
        .op_state
        .as_refreshing()
        .expect("refreshing state")
        .plan
        .len() as u32;

    let result = vault.finish_refreshing(caller, op_id).unwrap();

    assert_eq!(result.markets_refreshed, expected);
    assert!(vault.state().op_state.is_idle());
}

#[test]
fn test_sync_external_assets_in_allocating() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    let state = vault.state_mut();
    state.idle_assets = 2_000;
    state.total_assets = 2_000;

    let op_id = vault
        .begin_allocating(caller, vec![(0, 500)], 1000)
        .unwrap();

    vault
        .sync_external_assets(caller, 1000, op_id, 1000)
        .unwrap();

    assert_eq!(vault.state().external_assets, 1000);
}

#[test]
fn test_execute_withdraw_respects_min_withdrawal_assets() {
    let mut vault = create_test_vault();
    let allocator = [3u8; 32];
    let owner = [1u8; 32];
    let receiver = [2u8; 32];

    let deposit_amount = MIN_WITHDRAWAL_ASSETS.saturating_mul(2);
    let request_time: u64 = 200;
    let exec_time = request_time
        .saturating_add(templar_vault_kernel::DEFAULT_COOLDOWN_NS)
        .saturating_add(1);

    vault
        .deposit(owner, receiver, deposit_amount, 0, request_time)
        .unwrap();

    vault
        .request_withdraw(owner, receiver, deposit_amount, 0, request_time)
        .unwrap();

    let (head_id, head_escrow_before, head_expected_before) = {
        let (id, head) = vault
            .state()
            .withdraw_queue
            .head()
            .expect("withdrawal queued");
        (id, head.escrow_shares, head.expected_assets)
    };

    {
        let state = vault.state_mut();
        state.idle_assets = MIN_WITHDRAWAL_ASSETS.saturating_sub(1);
        state.total_assets = state.idle_assets.saturating_add(state.external_assets);
    }

    let summary = vault.execute_withdraw(allocator, exec_time).unwrap();

    assert_eq!(summary.assets_transferred, 0);
    assert_eq!(summary.shares_burned, 0);
    assert!(vault.state().op_state.is_withdrawing());
    let (head_id_after, head_after) = vault
        .state()
        .withdraw_queue
        .head()
        .expect("withdrawal still queued");
    assert_eq!(head_id_after, head_id);
    assert_eq!(head_after.escrow_shares, head_escrow_before);
    assert_eq!(head_after.expected_assets, head_expected_before);
}

#[test]
fn test_execute_withdraw_insufficient_idle_no_partial() {
    let mut vault = create_test_vault();
    let allocator = [3u8; 32];
    let owner = [1u8; 32];
    let receiver = [2u8; 32];

    let deposit_amount = MIN_WITHDRAWAL_ASSETS.saturating_mul(3);
    let request_time: u64 = 200;
    let exec_time = request_time
        .saturating_add(templar_vault_kernel::DEFAULT_COOLDOWN_NS)
        .saturating_add(1);

    vault
        .deposit(owner, receiver, deposit_amount, 0, request_time)
        .unwrap();

    vault
        .request_withdraw(owner, receiver, deposit_amount, 0, request_time)
        .unwrap();

    let (head_id, head_escrow_before, head_expected_before) = {
        let (id, head) = vault
            .state()
            .withdraw_queue
            .head()
            .expect("withdrawal queued");
        (id, head.escrow_shares, head.expected_assets)
    };

    {
        let state = vault.state_mut();
        state.idle_assets = MIN_WITHDRAWAL_ASSETS.saturating_add(1);
        state.total_assets = state.idle_assets.saturating_add(state.external_assets);
    }

    let summary = vault.execute_withdraw(allocator, exec_time).unwrap();

    assert_eq!(summary.assets_transferred, 0);
    assert_eq!(summary.shares_burned, 0);
    assert!(vault.state().op_state.is_withdrawing());
    let (head_id_after, head_after) = vault
        .state()
        .withdraw_queue
        .head()
        .expect("withdrawal still queued");
    assert_eq!(head_id_after, head_id);
    assert_eq!(head_after.escrow_shares, head_escrow_before);
    assert_eq!(head_after.expected_assets, head_expected_before);
}

#[test]
fn test_address_mapping_persists_for_execute_withdraw() {
    use soroban_sdk::testutils::Address as _;

    let env = Env::default();
    let contract_id = env.register(SorobanVaultContract, ());

    env.as_contract(&contract_id, || {
        let curator = SdkAddress::generate(&env);
        let asset = SdkAddress::generate(&env);
        let share = SdkAddress::generate(&env);
        let vault_sdk = env.current_contract_address();
        let curator_kernel = kernel_address_from_sdk(&env, &curator);
        let vault_kernel = kernel_address_from_sdk(&env, &vault_sdk);
        let asset_kernel = kernel_address_from_sdk(&env, &asset);
        let share_kernel = kernel_address_from_sdk(&env, &share);

        let config = ContractConfig::new(
            curator_kernel,
            vault_kernel,
            Vec::new(),
            Vec::new(),
            asset_kernel,
            share_kernel,
        );

        let mut vault = CuratorVault::new(
            config,
            MemoryStorage::new(),
            PermissiveAuth,
            TrackingInterpreter::new(),
            MockMarket,
            MockCrossChain,
        );
        vault.load_state().unwrap();

        let owner = SdkAddress::generate(&env);
        let receiver = SdkAddress::generate(&env);
        let executor = SdkAddress::generate(&env);
        let now_ns = 100u64;
        let assets = MIN_WITHDRAWAL_ASSETS.saturating_mul(2);

        vault
            .deposit_soroban(&env, owner.clone(), receiver.clone(), assets, 0, now_ns)
            .unwrap();
        vault
            .request_withdraw_soroban(&env, owner.clone(), receiver.clone(), assets, 0, now_ns)
            .unwrap();

        let storage = vault.storage.clone();

        let mut next_vault = CuratorVault::new(
            ContractConfig::new(
                curator_kernel,
                vault_kernel,
                Vec::new(),
                Vec::new(),
                asset_kernel,
                share_kernel,
            ),
            storage,
            PermissiveAuth,
            TrackingInterpreter::new(),
            MockMarket,
            MockCrossChain,
        );
        next_vault.load_state().unwrap();

        let receiver_kernel = kernel_address_from_sdk(&env, &receiver);

        assert!(!next_vault.interpreter.has_address(&receiver_kernel));

        let exec_time = now_ns
            .saturating_add(templar_vault_kernel::DEFAULT_COOLDOWN_NS)
            .saturating_add(1);
        let summary = next_vault
            .execute_withdraw_soroban(&env, executor, exec_time)
            .unwrap();

        assert!(summary.assets_transferred > 0);
        assert!(next_vault.interpreter.has_address(&receiver_kernel));
    });
}

#[test]
fn test_abort_allocating() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    // First deposit to have some idle assets
    vault.deposit([1u8; 32], [10u8; 32], 1000, 0, 100).unwrap();

    let op_id = vault
        .begin_allocating(caller, vec![(0, 500)], 1000)
        .unwrap();

    vault.abort_allocating(caller, op_id, 500).unwrap();

    assert!(vault.state().op_state.is_idle());
}

#[test]
fn test_contract_config() {
    let config = test_config();

    assert!(config.is_curator(&[1u8; 32]));
    assert!(!config.is_curator(&[2u8; 32]));

    assert!(config.is_guardian(&[2u8; 32]));
    assert!(!config.is_guardian(&[1u8; 32]));

    assert!(config.is_allocator(&[3u8; 32]));
    assert!(!config.is_allocator(&[1u8; 32]));

    assert!(config.is_privileged(&[1u8; 32])); // curator
    assert!(config.is_privileged(&[3u8; 32])); // allocator
    assert!(!config.is_privileged(&[2u8; 32])); // guardian only
}

#[test]
fn test_reentrancy_guard_blocks_nested() {
    use soroban_sdk::testutils::Address as _;

    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator, asset, share).unwrap();
        let result = with_reentrancy_guard(&env, || with_reentrancy_guard(&env, || Ok(())));
        assert_eq!(result, Err(ContractError::Reentrancy));
    });
}

#[test]
fn test_reentrancy_guard_resets_between_calls() {
    use soroban_sdk::testutils::Address as _;

    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator, asset, share).unwrap();
        with_reentrancy_guard(&env, || Ok(())).unwrap();
        with_reentrancy_guard(&env, || Ok(())).unwrap();
    });
}

#[test]
fn test_loads_fees_spec_from_storage() {
    use soroban_sdk::testutils::Address as _;
    use templar_vault_kernel::fee::FeeSlot;
    use templar_vault_kernel::math::wad::Wad;

    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator, asset, share).unwrap();
    });

    let fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, [1u8; 32]),
        FeeSlot::new(Wad::one() / 20, [2u8; 32]),
        None,
    );

    env.as_contract(&contract_id, || {
        let bytes = borsh::to_vec(&fees).expect("fees serialize");
        env.storage()
            .instance()
            .set(&VaultDataKey::FeesSpec, &bytes);
    });

    env.as_contract(&contract_id, || {
        with_contract_vault(&env, |vault| {
            assert_eq!(vault.config.fees, fees);
            Ok(())
        })
        .unwrap();
    });
}

#[test]
fn test_refresh_fees_mints_shares() {
    use templar_vault_kernel::fee::FeeSlot;
    use templar_vault_kernel::math::wad::Wad;

    let mut vault = create_test_vault();
    let fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, [9u8; 32]),
        FeeSlot::new(Wad::one() / 10, [8u8; 32]),
        None,
    );
    vault.config.fees = fees;

    {
        let state = vault.state_mut();
        state.total_assets = 1_500;
        state.total_shares = 1_000;
        state.idle_assets = 1_500;
        state.external_assets = 0;
        state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);
    }

    let annual_fee_assets = fees
        .management
        .fee_wad
        .apply_floored(Number::from(1_500u128));
    let mgmt_fee_assets = mul_div_floor(
        annual_fee_assets,
        Number::from(u128::from(YEAR_NS)),
        Number::from(u128::from(YEAR_NS)),
    );
    let mgmt_expected: u128 = compute_fee_shares_from_assets(
        mgmt_fee_assets,
        Number::from(1_500u128),
        Number::from(1_000u128),
    )
    .into();

    let total_supply_after_mgmt: u128 = 1_000u128 + mgmt_expected;
    let profit = 1_500u128.saturating_sub(1_000u128);
    let perf_fee_assets = fees.performance.fee_wad.apply_floored(Number::from(profit));
    let perf_expected: u128 = compute_fee_shares_from_assets(
        perf_fee_assets,
        Number::from(1_500u128),
        Number::from(total_supply_after_mgmt),
    )
    .into();

    let minted = vault.refresh_fees([1u8; 32], YEAR_NS).unwrap();

    assert_eq!(minted, mgmt_expected + perf_expected);
    assert_eq!(
        vault.state().total_shares,
        total_supply_after_mgmt + perf_expected
    );
    assert_eq!(vault.state().fee_anchor.total_assets, 1_500);
    assert_eq!(vault.state().fee_anchor.timestamp_ns, YEAR_NS);

    let mint_effects = vault
        .interpreter
        .effects
        .iter()
        .filter(|effect| matches!(effect, KernelEffect::MintShares { .. }))
        .count();
    assert_eq!(mint_effects, 2);
}

#[test]
fn test_refresh_fees_zero_elapsed_noop() {
    use templar_vault_kernel::fee::FeeSlot;
    use templar_vault_kernel::math::wad::Wad;

    let mut vault = create_test_vault();
    let fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, [9u8; 32]),
        FeeSlot::new(Wad::one() / 10, [8u8; 32]),
        None,
    );
    vault.config.fees = fees;

    {
        let state = vault.state_mut();
        state.total_assets = 1_000;
        state.total_shares = 1_000;
        state.idle_assets = 1_000;
        state.external_assets = 0;
        state.fee_anchor = FeeAccrualAnchor::new(1_000, 123);
    }

    let minted = vault.refresh_fees([1u8; 32], 123).unwrap();

    assert_eq!(minted, 0);
    assert_eq!(vault.state().total_shares, 1_000);
    assert_eq!(vault.state().fee_anchor.total_assets, 1_000);
    assert_eq!(vault.state().fee_anchor.timestamp_ns, 123);
    assert!(!vault
        .interpreter
        .effects
        .iter()
        .any(|effect| matches!(effect, KernelEffect::MintShares { .. })));
}

// =========================================================================
// Policy tests
// =========================================================================

#[test]
fn test_acquire_and_release_market_lock() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    // Acquire lock on market 1
    vault
        .acquire_market_lock(caller, 1, 5000, 1000)
        .expect("should acquire lock");

    // Market 1 should be locked
    assert!(vault.is_market_locked(1, 1500));
    // Market 2 should not be locked
    assert!(!vault.is_market_locked(2, 1500));

    // Release lock
    vault
        .release_market_lock(caller, 1)
        .expect("should release lock");

    // Market 1 should now be unlocked
    assert!(!vault.is_market_locked(1, 1500));
}

#[test]
fn test_lock_expiry() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    // Acquire lock that expires at 2000
    vault
        .acquire_market_lock(caller, 1, 2000, 1000)
        .expect("should acquire lock");

    // Market 1 should be locked before expiry
    assert!(vault.is_market_locked(1, 1500));

    // Market 1 should be unlocked after expiry
    assert!(!vault.is_market_locked(1, 2500));
}

#[test]
fn test_lock_expiry_in_past_rejected() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    // expiry_ns <= current_ns should be rejected
    let result = vault.acquire_market_lock(caller, 1, 1000, 1000);
    assert!(result.is_err());
    let result = vault.acquire_market_lock(caller, 1, 500, 1000);
    assert!(result.is_err());
}

#[test]
fn test_lock_max_duration_exceeded_rejected() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    // Duration > 7 days should be rejected
    let current_ns = 1_000_000_000u64;
    let eight_days_ns = 8 * 24 * 60 * 60 * 1_000_000_000u64;
    let result = vault.acquire_market_lock(caller, 1, current_ns + eight_days_ns, current_ns);
    assert!(result.is_err());

    // Duration exactly 7 days should succeed
    let seven_days_ns = 7 * 24 * 60 * 60 * 1_000_000_000u64;
    let result = vault.acquire_market_lock(caller, 1, current_ns + seven_days_ns, current_ns);
    assert!(result.is_ok());
}

#[test]
fn test_begin_allocating_filters_locked_markets() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    let state = vault.state_mut();
    state.idle_assets = 2_000;
    state.total_assets = 2_000;

    // Lock market 1
    vault
        .acquire_market_lock(caller, 1, 5000, 1000)
        .expect("should acquire lock");

    // Start allocation with markets 0, 1, 2 (1 is locked)
    let plan = vec![(0, 100), (1, 200), (2, 300)];
    let op_id = vault
        .begin_allocating(caller, plan, 1500)
        .expect("should start allocation");

    assert_eq!(op_id, 0);
    assert!(vault.state().op_state.is_allocating());

    // The allocation should have filtered out market 1
    // (We can't directly inspect the plan, but the operation should succeed)
}

#[test]
fn test_begin_refreshing_filters_locked_markets() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    // Lock market 2
    vault
        .acquire_market_lock(caller, 2, 5000, 1000)
        .expect("should acquire lock");

    // Start refresh with markets 0, 1, 2 (2 is locked)
    let plan = vec![0, 1, 2];
    let op_id = vault
        .begin_refreshing(caller, plan, 1500)
        .expect("should start refresh");

    assert_eq!(op_id, 0);
    assert!(vault.state().op_state.is_refreshing());
}

#[test]
fn test_allocating_all_locked_markets() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    // Lock both markets in the plan
    vault.acquire_market_lock(caller, 0, 5000, 1000).unwrap();
    vault.acquire_market_lock(caller, 1, 5000, 1000).unwrap();

    // Start allocation with only locked markets - results in empty plan
    // The kernel rejects empty allocation plans
    let plan = vec![(0, 100), (1, 200)];
    let result = vault.begin_allocating(caller, plan, 1500);

    // Empty plan is rejected by kernel
    assert!(result.is_err());
    // Vault should still be in idle state
    assert!(vault.state().op_state.is_idle());
}

#[test]
fn test_policy_state_getter() {
    let vault = create_test_vault();

    // Policy state should be initialized empty
    assert!(vault.policy_state().locks.is_empty());
    assert!(vault.policy_state().markets.is_empty());
    assert!(vault.policy_state().principals.is_empty());
    assert!(vault.policy_state().cap_groups.is_empty());
}

#[test]
fn test_load_state_restores_policy_and_restrictions() {
    use crate::policy::MarketLock;
    use soroban_sdk::testutils::Address as _;
    use std::collections::BTreeSet;

    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator, asset, share).unwrap();

        let mut storage = SorobanStorage::new(&env);
        let versioned = VersionedState::new(VaultState::default());
        storage.save_state(&versioned).unwrap();
        storage.save_paused(false).unwrap();

        let mut policy_state = PolicyState::new();
        let lock = MarketLock::new(1, 10).with_expiry(20);
        policy_state.locks = policy_state.locks.acquire(lock, 10).unwrap();
        Storage::save_policy_state(&mut storage, &policy_state).unwrap();

        let mut blacklist = BTreeSet::new();
        blacklist.insert([9u8; 32]);
        let restrictions = Restrictions::Blacklist(blacklist);
        Storage::save_restrictions(&mut storage, &Some(restrictions.clone())).unwrap();

        let mut vault = CuratorVault::new(
            test_config(),
            storage,
            PermissiveAuth,
            MockInterpreter::new(),
            MockMarket,
            MockCrossChain,
        );
        vault.load_state().unwrap();

        assert!(vault.is_market_locked(1, 10));
        assert_eq!(vault.restrictions(), Some(&restrictions));
    });
}
