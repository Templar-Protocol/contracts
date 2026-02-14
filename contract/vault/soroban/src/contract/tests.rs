use super::*;
use crate::auth::PermissiveAuth;
use crate::effects::{AddressRegistrar, EffectContext, EffectInterpreter, EffectResult};
use crate::storage::{MemoryStorage, Storage, VersionedState};
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

fn create_test_vault() -> CuratorVault<MemoryStorage, PermissiveAuth, MockInterpreter> {
    let mut vault = CuratorVault::new(
        test_config(),
        MemoryStorage::new(),
        PermissiveAuth,
        MockInterpreter::new(),
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

    let state = vault.state_mut().unwrap();
    state.idle_assets = 2_000;
    state.total_assets = 2_000;

    let op_id = vault
        .begin_allocating(caller, vec![(0, 500), (1, 500)], 1000)
        .unwrap();

    assert_eq!(op_id, 0);
    assert!(vault.state().unwrap().op_state.is_allocating());
}

#[test]
fn test_finish_allocating() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    let state = vault.state_mut().unwrap();
    state.idle_assets = 2_000;
    state.total_assets = 2_000;

    let op_id = vault
        .begin_allocating(caller, vec![(0, 500)], 1000)
        .unwrap();

    let result = vault.finish_allocating(caller, op_id).unwrap();

    assert_eq!(result.op_id, op_id);
    assert!(vault.state().unwrap().op_state.is_idle());
}

#[test]
fn test_begin_refreshing() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    let op_id = vault.begin_refreshing(caller, vec![0, 1], 1000).unwrap();

    assert_eq!(op_id, 0);
    assert!(vault.state().unwrap().op_state.is_refreshing());
}

#[test]
fn test_finish_refreshing_reports_markets_refreshed() {
    let mut vault = create_test_vault();
    let caller = [3u8; 32]; // allocator

    let op_id = vault
        .begin_refreshing(caller, vec![0, 1, 2], 1500)
        .expect("should start refresh");

    let expected = vault
        .state()
        .unwrap()
        .op_state
        .as_refreshing()
        .expect("refreshing state")
        .plan
        .len() as u32;

    let result = vault.finish_refreshing(caller, op_id).unwrap();

    assert_eq!(result.markets_refreshed, expected);
    assert!(vault.state().unwrap().op_state.is_idle());
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
            .unwrap()
            .withdraw_queue
            .head()
            .expect("withdrawal queued");
        (id, head.escrow_shares, head.expected_assets)
    };

    {
        let state = vault.state_mut().unwrap();
        state.idle_assets = MIN_WITHDRAWAL_ASSETS.saturating_sub(1);
        state.total_assets = state.idle_assets.saturating_add(state.external_assets);
    }

    let summary = vault.execute_withdraw(allocator, exec_time).unwrap();

    assert_eq!(summary.assets_transferred, 0);
    assert_eq!(summary.shares_burned, 0);
    assert!(vault.state().unwrap().op_state.is_withdrawing());
    let (head_id_after, head_after) = vault
        .state()
        .unwrap()
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
            .unwrap()
            .withdraw_queue
            .head()
            .expect("withdrawal queued");
        (id, head.escrow_shares, head.expected_assets)
    };

    {
        let state = vault.state_mut().unwrap();
        state.idle_assets = MIN_WITHDRAWAL_ASSETS.saturating_add(1);
        state.total_assets = state.idle_assets.saturating_add(state.external_assets);
    }

    let summary = vault.execute_withdraw(allocator, exec_time).unwrap();

    assert_eq!(summary.assets_transferred, 0);
    assert_eq!(summary.shares_burned, 0);
    assert!(vault.state().unwrap().op_state.is_withdrawing());
    let (head_id_after, head_after) = vault
        .state()
        .unwrap()
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
        SorobanVaultContract::initialize(env.clone(), curator.clone(), curator, asset, share)
            .unwrap();
        let result =
            with_reentrancy_guard(&env, &mut || with_reentrancy_guard(&env, &mut || Ok(())));
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
        SorobanVaultContract::initialize(env.clone(), curator.clone(), curator, asset, share)
            .unwrap();
        with_reentrancy_guard(&env, &mut || Ok(())).unwrap();
        with_reentrancy_guard(&env, &mut || Ok(())).unwrap();
    });
}

#[test]
fn test_reentrancy_guard_blocks_read_only_entrypoints() {
    use soroban_sdk::testutils::Address as _;

    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator.clone(), curator, asset, share)
            .unwrap();
        with_reentrancy_guard(&env, &mut || {
            let result = SorobanVaultContract::vault_snapshot(env.clone());
            assert!(
                matches!(result, Err(ContractError::Reentrancy)),
                "expected Reentrancy error, got: {result:?}"
            );
            Ok(())
        })
        .unwrap();
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
        SorobanVaultContract::initialize(env.clone(), curator.clone(), curator, asset, share)
            .unwrap();
    });

    let fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, [1u8; 32]),
        FeeSlot::new(Wad::one() / 20, [2u8; 32]),
        None,
    );

    env.as_contract(&contract_id, || {
        let bytes = postcard::to_allocvec(&fees).expect("fees serialize");
        env.storage()
            .instance()
            .set(&VaultDataKey::FeesSpec, &bytes);
    });

    env.as_contract(&contract_id, || {
        let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
            assert_eq!(vault.config.fees, fees);
            Ok(())
        };
        with_contract_vault(&env, &mut call).unwrap();
    });
}

#[test]
fn test_atomic_withdraw_refreshes_fees() {
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::token::StellarAssetClient;
    use templar_vault_kernel::fee::FeeSlot;
    use templar_vault_kernel::math::wad::Wad;

    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 23,
        ..Default::default()
    });

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);

    let asset_admin = soroban_sdk::Address::generate(&env);
    let asset_sac = env.register_stellar_asset_contract_v2(asset_admin.clone());
    let asset = asset_sac.address();
    let asset_admin_client = StellarAssetClient::new(&env, &asset);

    let share_sac = env.register_stellar_asset_contract_v2(contract_id.clone());
    let share = share_sac.address();
    let share_admin_client = StellarAssetClient::new(&env, &share);

    let owner = soroban_sdk::Address::generate(&env);
    let receiver = soroban_sdk::Address::generate(&env);
    let operator = soroban_sdk::Address::generate(&env);
    let mgmt_recipient = soroban_sdk::Address::generate(&env);
    let perf_recipient = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(
            env.clone(),
            curator.clone(),
            curator,
            asset.clone(),
            share.clone(),
        )
        .unwrap();

        let fees = FeesSpec::new(
            FeeSlot::new(
                Wad::one() / 10,
                kernel_address_from_sdk(&env, &perf_recipient),
            ),
            FeeSlot::new(
                Wad::one() / 10,
                kernel_address_from_sdk(&env, &mgmt_recipient),
            ),
            None,
        );
        let bytes = postcard::to_allocvec(&fees).expect("fees serialize");
        env.storage()
            .instance()
            .set(&VaultDataKey::FeesSpec, &bytes);

        let mut storage = SorobanStorage::new(&env);
        storage.save_address(
            &kernel_address_from_sdk(&env, &mgmt_recipient),
            &mgmt_recipient,
        );
        storage.save_address(
            &kernel_address_from_sdk(&env, &perf_recipient),
            &perf_recipient,
        );

        let mut state = VaultState::default();
        state.total_assets = 1_500;
        state.total_shares = 1_000;
        state.idle_assets = 1_500;
        state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);
        storage
            .save_state(&VersionedState::new(state))
            .expect("save state");
    });

    asset_admin_client.mint(&contract_id, &1_500);
    share_admin_client.mint(&owner, &1_000);

    let burned = env
        .as_contract(&contract_id, || {
            SorobanVaultContract::withdraw(env.clone(), 500, receiver, owner.clone(), operator)
        })
        .expect("withdraw should succeed");
    assert!(burned > 0);

    let share_client = soroban_sdk::token::Client::new(&env, &share);
    assert!(share_client.balance(&perf_recipient) > 0);

    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);
        let versioned = storage.load_state().unwrap().expect("state");
        assert_eq!(versioned.state.fee_anchor.total_assets, 1_500);
        assert_eq!(
            versioned.state.fee_anchor.timestamp_ns,
            ledger_timestamp_ns(&env).expect("timestamp")
        );
    });
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
    use soroban_sdk::testutils::Address as _;

    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset = soroban_sdk::Address::generate(&env);
    let share = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(env.clone(), curator.clone(), curator, asset, share)
            .unwrap();

        let mut storage = SorobanStorage::new(&env);
        let versioned = VersionedState::new(VaultState::default());
        storage.save_state(&versioned).unwrap();
        storage.save_paused(false).unwrap();

        Storage::save_policy_state(&mut storage, &PolicyState::new()).unwrap();

        let restrictions = Restrictions::Blacklist(alloc::vec![[9u8; 32]]);
        Storage::save_restrictions(&mut storage, &Some(restrictions.clone())).unwrap();

        let mut vault = CuratorVault::new(
            test_config(),
            storage,
            PermissiveAuth,
            MockInterpreter::new(),
        );
        vault.load_state().unwrap();

        assert_eq!(vault.restrictions(), Some(&restrictions));
    });
}
