// Kitchen-sink unit tests for Soroban vault runtime.

mod auth_tests {
    use crate::auth::{ActionKind, AuthError, SorobanAuth};
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address as SdkAddress, Env};
    use templar_curator_primitives::rbac::Role;

    #[test]
    fn test_soroban_auth_new() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);

        let auth = SorobanAuth::new(&env, curator.clone());

        assert_eq!(auth.curator(), &curator);
        assert!(!auth.paused());
    }

    #[test]
    fn test_soroban_auth_curator_role() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::new(&env, curator.clone());

        assert!(auth.has_role(Role::Curator, &curator));
        assert!(!auth.has_role(Role::Curator, &user));
    }

    #[test]
    fn test_soroban_auth_sentinel_role() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let sentinel = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), Some(sentinel.clone()), None);

        assert!(auth.has_role(Role::Sentinel, &curator));
        assert!(auth.has_role(Role::Sentinel, &sentinel));
        assert!(!auth.has_role(Role::Sentinel, &user));
    }

    #[test]
    fn test_soroban_auth_allocator_role() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));

        // Curator is always an allocator
        assert!(auth.has_role(Role::Allocator, &curator));
        // Designated allocator
        assert!(auth.has_role(Role::Allocator, &allocator));
        // Regular user is not
        assert!(!auth.has_role(Role::Allocator, &user));
    }

    #[test]
    fn test_soroban_auth_check_role_user_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::new(&env, curator);

        // User actions allowed for anyone
        assert!(auth.check_role(ActionKind::Deposit, &user).is_ok());
        assert!(auth.check_role(ActionKind::RequestWithdraw, &user).is_ok());

        let result = auth.check_role(ActionKind::ExecuteWithdraw, &user);
        assert!(matches!(result, Err(AuthError::MissingRole)));
    }

    #[test]
    fn test_soroban_auth_check_role_pause_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let sentinel = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), Some(sentinel.clone()), None);

        // Sentinel can pause
        assert!(auth.check_role(ActionKind::Pause, &sentinel).is_ok());
        // Curator can pause
        assert!(auth.check_role(ActionKind::Pause, &curator).is_ok());
        // User cannot pause
        let result = auth.check_role(ActionKind::Pause, &user);
        assert!(matches!(result, Err(AuthError::MissingRole)));

        assert!(auth
            .check_role(ActionKind::SetRestrictions, &sentinel)
            .is_ok());
    }

    #[test]
    fn test_soroban_auth_check_role_allocator_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));

        // Allocator can do allocation operations
        assert!(auth
            .check_role(ActionKind::BeginAllocating, &allocator)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::FinishAllocating, &allocator)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::BeginRefreshing, &allocator)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::SyncExternalAssets, &allocator)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::ExecuteWithdraw, &allocator)
            .is_ok());

        // Curator can too
        assert!(auth
            .check_role(ActionKind::BeginAllocating, &curator)
            .is_ok());

        // User cannot
        let result = auth.check_role(ActionKind::BeginAllocating, &user);
        assert!(matches!(result, Err(AuthError::MissingRole)));
    }

    #[test]
    fn test_soroban_auth_check_role_allocator_emergency_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let sentinel = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(
            &env,
            curator.clone(),
            Some(sentinel.clone()),
            Some(allocator.clone()),
        );

        assert!(auth
            .check_role(ActionKind::AbortWithdrawing, &allocator)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::AbortWithdrawing, &sentinel)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::AbortWithdrawing, &curator)
            .is_ok());

        let result = auth.check_role(ActionKind::AbortWithdrawing, &user);
        assert!(matches!(result, Err(AuthError::MissingRole)));
    }

    #[test]
    fn test_soroban_auth_check_role_curator_only() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));

        // Only curator can do manual reconcile
        assert!(auth
            .check_role(ActionKind::ManualReconcile, &curator)
            .is_ok());

        // Allocator cannot
        let result = auth.check_role(ActionKind::ManualReconcile, &allocator);
        assert!(matches!(result, Err(AuthError::MissingRole)));

        assert!(auth.check_role(ActionKind::PolicyAdmin, &curator).is_ok());
        let result = auth.check_role(ActionKind::PolicyAdmin, &allocator);
        assert!(matches!(result, Err(AuthError::MissingRole)));
    }

    #[test]
    fn test_soroban_auth_paused_allows_privileged_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);

        let mut auth =
            SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));
        auth.set_paused(true);

        assert!(auth
            .check_role(ActionKind::BeginAllocating, &allocator)
            .is_ok());
        assert!(auth.check_role(ActionKind::PolicyAdmin, &curator).is_ok());
    }

    #[test]
    fn test_soroban_auth_set_paused() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);

        let mut auth = SorobanAuth::new(&env, curator);

        assert!(!auth.paused());
        auth.set_paused(true);
        assert!(auth.paused());
        auth.set_paused(false);
        assert!(!auth.paused());
    }
}

mod contract_tests {
    use crate::auth::{ActionKind, AuthAdapter, AuthResult};
    use crate::contract::*;
    use crate::convert::ledger_timestamp_ns;
    use crate::effects::{AddressRegistrar, EffectContext, EffectInterpreter, EffectResult};
    use crate::error::RuntimeError;
    use crate::storage::{MemoryStorage, SorobanStorage, Storage, VersionedState};
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use alloc::vec::Vec;
    use soroban_sdk::{Address as SdkAddress, Bytes, Env};
    use templar_curator_primitives::PolicyState;
    use templar_vault_kernel::effects::KernelEffect;
    use templar_vault_kernel::{
        FeeAccrualAnchor, FeesSpec, Restrictions, VaultState, MIN_WITHDRAWAL_ASSETS,
    };

    #[derive(Clone, Copy, Default)]
    struct TestPermissiveAuth;

    impl AuthAdapter for TestPermissiveAuth {
        fn authorize(
            &self,
            _action: ActionKind,
            _caller: [u8; 32],
            _proof: Option<&[u8]>,
        ) -> AuthResult<()> {
            Ok(())
        }

        fn is_paused(&self) -> bool {
            false
        }
    }

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
        fn execute_effect(
            &mut self,
            effect: &KernelEffect,
            _ctx: &EffectContext,
        ) -> EffectResult<()> {
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
        fn execute_effect(
            &mut self,
            effect: &KernelEffect,
            _ctx: &EffectContext,
        ) -> EffectResult<()> {
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

    fn create_test_vault() -> CuratorVault<MemoryStorage, TestPermissiveAuth, MockInterpreter> {
        let mut vault = CuratorVault::new(
            test_config(),
            MemoryStorage::new(),
            TestPermissiveAuth,
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
                TestPermissiveAuth,
                TrackingInterpreter::new(),
            );
            vault.load_state().unwrap();

            let owner = SdkAddress::generate(&env);
            let receiver = SdkAddress::generate(&env);
            let executor = SdkAddress::generate(&env);
            let now_ns = 100u64;
            let assets = MIN_WITHDRAWAL_ASSETS.saturating_mul(2);

            vault
                .deposit_mapped(&env, owner.clone(), receiver.clone(), assets, 0, now_ns)
                .unwrap();
            vault
                .request_withdraw_mapped(&env, owner.clone(), receiver.clone(), assets, 0, now_ns)
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
                TestPermissiveAuth,
                TrackingInterpreter::new(),
            );
            next_vault.load_state().unwrap();

            let receiver_kernel = kernel_address_from_sdk(&env, &receiver);

            assert!(!next_vault.interpreter.has_address(&receiver_kernel));

            let exec_time = now_ns
                .saturating_add(templar_vault_kernel::DEFAULT_COOLDOWN_NS)
                .saturating_add(1);
            let summary = next_vault
                .execute_withdraw_mapped(&env, executor, exec_time)
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
    fn test_loads_fees_spec_from_storage() {
        use soroban_sdk::testutils::Address as _;
        use templar_vault_kernel::fee::FeeSlot;
        use templar_vault_kernel::math::wad::Wad;

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

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
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set(LedgerInfo {
            timestamp: 100,
            protocol_version: 25,
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
        let operator = owner.clone();
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
    fn test_atomic_withdraw_requires_allowance_for_delegated_operator() {
        use soroban_sdk::testutils::Address as _;
        use soroban_sdk::token::StellarAssetClient;
        use soroban_sdk::IntoVal;

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

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

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                curator,
                asset.clone(),
                share.clone(),
            )
            .unwrap();

            let mut storage = SorobanStorage::new(&env);
            let mut state = VaultState::default();
            state.total_assets = 1_500;
            state.total_shares = 1_000;
            state.idle_assets = 1_500;
            storage
                .save_state(&VersionedState::new(state))
                .expect("save state");
        });

        asset_admin_client.mint(&contract_id, &1_500);
        share_admin_client.mint(&owner, &1_000);

        let without_approval = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.as_contract(&contract_id, || {
                SorobanVaultContract::withdraw(
                    env.clone(),
                    500,
                    receiver.clone(),
                    owner.clone(),
                    operator.clone(),
                )
            })
        }));
        assert!(without_approval.is_err());

        env.invoke_contract::<()>(
            &share,
            &soroban_sdk::Symbol::new(&env, "approve"),
            (&owner, &operator, &1_000i128, &1_000_000u32).into_val(&env),
        );

        let burned = env
            .as_contract(&contract_id, || {
                SorobanVaultContract::withdraw(
                    env.clone(),
                    500,
                    receiver.clone(),
                    owner.clone(),
                    operator.clone(),
                )
            })
            .expect("delegated withdraw should succeed with approval");
        assert!(burned > 0);

        let remaining_allowance: i128 = env.invoke_contract(
            &share,
            &soroban_sdk::Symbol::new(&env, "allowance"),
            (&owner, &operator).into_val(&env),
        );
        assert!(remaining_allowance < 1_000);
    }

    #[test]
    fn test_phase1_deposit_with_min_resource_probe() {
        use soroban_sdk::testutils::Address as _;
        use soroban_sdk::token::StellarAssetClient;

        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SorobanVaultContract, ());
        let curator = soroban_sdk::Address::generate(&env);

        let asset_admin = soroban_sdk::Address::generate(&env);
        let asset_sac = env.register_stellar_asset_contract_v2(asset_admin.clone());
        let asset = asset_sac.address();
        let asset_admin_client = StellarAssetClient::new(&env, &asset);

        let share_sac = env.register_stellar_asset_contract_v2(contract_id.clone());
        let share = share_sac.address();

        let owner = soroban_sdk::Address::generate(&env);
        let receiver = soroban_sdk::Address::generate(&env);
        let deposit_assets = 1_000_000_i128;

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                curator.clone(),
                asset.clone(),
                share.clone(),
            )
            .unwrap();
        });

        asset_admin_client.mint(&owner, &deposit_assets);

        let asset_client = soroban_sdk::token::Client::new(&env, &asset);
        let share_client = soroban_sdk::token::Client::new(&env, &share);
        let owner_assets_before = asset_client.balance(&owner);

        env.cost_estimate().budget().reset_default();
        let minted = env
            .as_contract(&contract_id, || {
                SorobanVaultContract::deposit_with_min(
                    env.clone(),
                    owner.clone(),
                    receiver.clone(),
                    deposit_assets,
                    0,
                )
            })
            .expect("deposit_with_min should succeed");
        let resources = env.cost_estimate().resources();

        std::println!(
        "phase1 real deposit probe: assets_in={} shares_out={} instructions={} mem_bytes={} writes={} read_entries={}",
        deposit_assets,
        minted,
        resources.instructions,
        resources.mem_bytes,
        resources.write_entries,
        resources.disk_read_entries + resources.memory_read_entries
    );

        assert!(minted > 0);
        assert_eq!(share_client.balance(&receiver), minted);
        assert_eq!(
            asset_client.balance(&owner),
            owner_assets_before - deposit_assets
        );
        assert_eq!(asset_client.balance(&contract_id), deposit_assets);
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

            Storage::save_policy_state(&mut storage, &PolicyState::default()).unwrap();

            let restrictions = Restrictions::Blacklist(alloc::vec![[9u8; 32]]);
            Storage::save_restrictions(&mut storage, &Some(restrictions.clone())).unwrap();

            let mut vault = CuratorVault::new(
                test_config(),
                storage,
                TestPermissiveAuth,
                MockInterpreter::new(),
            );
            vault.load_state().unwrap();

            assert_eq!(vault.restrictions(), Some(&restrictions));
        });
    }
}

mod convert_tests {
    use crate::convert::{
        ledger_timestamp_ns, runtime_to_contract, to_i128, to_u128, u128_to_i128_effect,
    };
    use crate::error::{ContractError, RuntimeError};
    use soroban_sdk::testutils::{Ledger as _, LedgerInfo};
    use soroban_sdk::Env;

    #[test]
    fn to_i128_converts_in_range() {
        assert_eq!(to_i128(0).expect("zero must convert"), 0);
        assert_eq!(
            to_i128(i128::MAX as u128).expect("max must convert"),
            i128::MAX
        );
    }

    #[test]
    fn to_i128_rejects_overflow() {
        assert_eq!(
            to_i128((i128::MAX as u128) + 1).expect_err("overflow must fail"),
            ContractError::ConversionOverflow
        );
    }

    #[test]
    fn to_u128_rejects_negative() {
        assert_eq!(
            to_u128(-1).expect_err("negative must fail"),
            ContractError::InvalidInput
        );
    }

    #[test]
    fn u128_to_i128_effect_sets_effect_error() {
        let err = u128_to_i128_effect((i128::MAX as u128) + 1, "event amount overflow")
            .expect_err("overflow must fail");
        assert_eq!(err, RuntimeError::effect_failed("event amount overflow"));
    }

    #[test]
    fn runtime_to_contract_maps_error() {
        let err =
            runtime_to_contract::<()>(Err(RuntimeError::InvalidInput)).expect_err("error must map");
        assert_eq!(err, ContractError::InvalidInput);
    }

    #[test]
    fn ledger_timestamp_converts_to_ns() {
        let env = Env::default();
        env.ledger().set(LedgerInfo {
            timestamp: 123,
            protocol_version: 25,
            ..Default::default()
        });

        assert_eq!(
            ledger_timestamp_ns(&env).expect("timestamp conversion must succeed"),
            123_000_000_000
        );
    }
}

mod effects_tests {
    use crate::effects::*;
    use crate::error::RuntimeError;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address, Env};
    use templar_vault_kernel::effects::KernelEffect;

    #[derive(Clone, Debug, Default)]
    struct TestSep41Token {
        should_fail: bool,
        mock_balance: i128,
    }

    impl TestSep41Token {
        fn new() -> Self {
            Self {
                should_fail: false,
                mock_balance: 1000,
            }
        }

        fn failing() -> Self {
            Self {
                should_fail: true,
                mock_balance: 0,
            }
        }
    }

    impl Sep41Token for TestSep41Token {
        fn mint(&self, _to: &Address, _amount: i128) -> EffectResult<()> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test mint failed"));
            }
            Ok(())
        }

        fn burn(&self, _from: &Address, _amount: i128) -> EffectResult<()> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test burn failed"));
            }
            Ok(())
        }

        fn burn_from(
            &self,
            _spender: &Address,
            _from: &Address,
            _amount: i128,
        ) -> EffectResult<()> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test burn_from failed"));
            }
            Ok(())
        }

        fn transfer(&self, _from: &Address, _to: &Address, _amount: i128) -> EffectResult<()> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test transfer failed"));
            }
            Ok(())
        }

        fn balance(&self, _addr: &Address) -> EffectResult<i128> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test balance failed"));
            }
            Ok(self.mock_balance)
        }
    }

    fn test_env() -> Env {
        Env::default()
    }

    fn test_context() -> EffectContext {
        EffectContext::new(1_000_000_000_000, [1u8; 32], [2u8; 32], [3u8; 32])
    }

    #[test]
    fn test_effect_summary_new() {
        let summary = EffectSummary::new();
        assert_eq!(summary.shares_minted, 0);
        assert_eq!(summary.shares_burned, 0);
        assert_eq!(summary.shares_transferred, 0);
        assert_eq!(summary.assets_transferred, 0);
        assert_eq!(summary.events_emitted, 0);
    }

    #[test]
    fn test_effect_summary_recording() {
        let mut summary = EffectSummary::new();

        summary.record_mint(100);
        assert_eq!(summary.shares_minted, 100);

        summary.record_burn(50);
        assert_eq!(summary.shares_burned, 50);

        summary.record_share_transfer(25);
        assert_eq!(summary.shares_transferred, 25);

        summary.record_asset_transfer(1000);
        assert_eq!(summary.assets_transferred, 1000);

        summary.record_event();
        summary.record_event();
        assert_eq!(summary.events_emitted, 2);
    }

    #[test]
    fn test_effect_context_new() {
        let ctx = test_context();
        assert_eq!(ctx.now_ns, 1_000_000_000_000);
        assert_eq!(ctx.vault_address, [1u8; 32]);
        assert_eq!(ctx.asset_address, [2u8; 32]);
        assert_eq!(ctx.share_address, [3u8; 32]);
    }

    #[test]
    fn test_test_sep41_token_mint() {
        let env = test_env();
        let token = TestSep41Token::new();
        let addr = Address::generate(&env);
        let result = token.mint(&addr, 100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_test_sep41_token_burn() {
        let env = test_env();
        let token = TestSep41Token::new();
        let addr = Address::generate(&env);
        let result = token.burn(&addr, 50);
        assert!(result.is_ok());
    }

    #[test]
    fn test_test_sep41_token_transfer() {
        let env = test_env();
        let token = TestSep41Token::new();
        let from = Address::generate(&env);
        let to = Address::generate(&env);
        let result = token.transfer(&from, &to, 25);
        assert!(result.is_ok());
    }

    #[test]
    fn test_test_sep41_token_balance() {
        let env = test_env();
        let token = TestSep41Token::new();
        let addr = Address::generate(&env);
        let result = token.balance(&addr);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1000);
    }

    #[test]
    fn test_test_sep41_token_failing() {
        let env = test_env();
        let token = TestSep41Token::failing();
        let addr = Address::generate(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);

        assert!(token.mint(&addr, 100).is_err());
        assert!(token.burn(&addr, 50).is_err());
        assert!(token.transfer(&from, &to, 25).is_err());
        assert!(token.balance(&addr).is_err());
    }

    #[test]
    fn test_u128_to_i128_conversion() {
        // Valid conversions
        assert!(to_i128_event(0).is_ok());
        assert!(to_i128_event(1000).is_ok());
        assert!(to_i128_event(i128::MAX as u128).is_ok());

        // Overflow
        assert!(to_i128_event((i128::MAX as u128) + 1).is_err());
    }

    #[test]
    fn test_address_map() {
        let env = test_env();
        let mut map = AddressMap::new(&env);

        let kernel_addr = [1u8; 32];
        let soroban_addr = Address::generate(&env);

        map.register(kernel_addr, soroban_addr.clone());

        let resolved = map.resolve(&kernel_addr);
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap(), &soroban_addr);

        // Unknown address
        let unknown = [2u8; 32];
        assert!(map.resolve(&unknown).is_none());
    }

    #[test]
    fn test_emit_event_serializes_without_address_mapping() {
        use templar_vault_kernel::effects::KernelEvent;

        let env = test_env();
        let share = TestSep41Token::new();
        let asset = TestSep41Token::new();
        let mut interpreter = SorobanEffectInterpreter::new(&env, &share, &asset);
        let ctx = test_context();

        let effect = KernelEffect::EmitEvent {
            event: KernelEvent::DepositProcessed {
                owner: [1u8; 32],
                receiver: [2u8; 32],
                assets_in: 1,
                shares_out: 1,
            },
        };

        assert!(interpreter.execute_effect(&effect, &ctx).is_ok());
    }
}

mod market_tests {
    use crate::error::RuntimeError;
    use crate::market::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address, Bytes, Env};
    use templar_vault_kernel::AssetId;

    #[derive(Clone, Default)]
    struct TestMarketAdapter {
        mock_total_assets: i128,
        should_fail: bool,
    }

    impl TestMarketAdapter {
        const fn new(mock_total_assets: i128) -> Self {
            Self {
                mock_total_assets,
                should_fail: false,
            }
        }

        const fn failing() -> Self {
            Self {
                mock_total_assets: 0,
                should_fail: true,
            }
        }
    }

    impl TestMarketAdapter {
        fn supply(&self, _env: &Env, _asset: &Address, _amount: i128) -> Result<(), RuntimeError> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test supply failed"));
            }
            Ok(())
        }

        fn withdraw(
            &self,
            _env: &Env,
            _asset: &Address,
            _amount: i128,
        ) -> Result<(), RuntimeError> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test withdraw failed"));
            }
            Ok(())
        }

        fn progress_withdrawal(
            &self,
            _env: &Env,
            _asset: &Address,
            amount: i128,
        ) -> Result<i128, RuntimeError> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed(
                    "test progress_withdrawal failed",
                ));
            }
            Ok(amount)
        }

        fn total_assets(&self, _env: &Env, _asset: &Address) -> Result<i128, RuntimeError> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test total_assets failed"));
            }
            Ok(self.mock_total_assets)
        }
    }

    #[derive(Clone, Default)]
    struct TestCrossChainAdapter {
        next_attempt_id: u64,
        settlement_receipt: Option<SettlementReceipt>,
        mock_total_assets: i128,
        should_fail: bool,
    }

    impl TestCrossChainAdapter {
        const fn new() -> Self {
            Self {
                next_attempt_id: 1,
                settlement_receipt: None,
                mock_total_assets: 0,
                should_fail: false,
            }
        }

        fn with_settlement(mut self, receipt: SettlementReceipt) -> Self {
            self.settlement_receipt = Some(receipt);
            self
        }
    }

    impl SorobanCrossChainMarketAdapter for TestCrossChainAdapter {
        fn submit_intent(&self, _env: &Env, _plan_bytes: Bytes) -> Result<u64, RuntimeError> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test submit_intent failed"));
            }
            Ok(self.next_attempt_id)
        }

        fn settle(
            &self,
            _env: &Env,
            op_id: u64,
            attempt_id: u64,
        ) -> Result<SettlementReceipt, RuntimeError> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test settle failed"));
            }
            Ok(self
                .settlement_receipt
                .clone()
                .unwrap_or(SettlementReceipt::new(
                    op_id,
                    attempt_id,
                    self.mock_total_assets,
                )))
        }

        fn total_assets(&self, _env: &Env, _asset: &Address) -> Result<i128, RuntimeError> {
            if self.should_fail {
                return Err(RuntimeError::effect_failed("test total_assets failed"));
            }
            Ok(self.mock_total_assets)
        }
    }

    #[test]
    fn test_settlement_receipt_new() {
        let receipt = SettlementReceipt::new(1, 2, 1000);
        assert_eq!(receipt.op_id, 1);
        assert_eq!(receipt.attempt_id, 2);
        assert_eq!(receipt.new_external_assets, 1000);
    }

    #[test]
    fn test_market_ref_new() {
        let asset = AssetId::from([7u8; 32]);
        let market_ref: MarketRef = (42, asset.clone()).into();
        assert_eq!(market_ref.market_id, 42);
        assert_eq!(market_ref.asset_id, asset);
    }

    #[test]
    fn test_test_market_adapter_success() {
        let adapter = TestMarketAdapter::new(1000);
        let env = Env::default();
        let asset = Address::generate(&env);

        assert!(adapter.supply(&env, &asset, 100).is_ok());
        assert!(adapter.withdraw(&env, &asset, 50).is_ok());
        assert_eq!(adapter.progress_withdrawal(&env, &asset, 25).unwrap(), 25);
        assert_eq!(adapter.total_assets(&env, &asset).unwrap(), 1000);
    }

    #[test]
    fn test_test_market_adapter_failure() {
        let adapter = TestMarketAdapter::failing();
        let env = Env::default();
        let asset = Address::generate(&env);

        assert!(adapter.supply(&env, &asset, 100).is_err());
        assert!(adapter.withdraw(&env, &asset, 50).is_err());
        assert!(adapter.progress_withdrawal(&env, &asset, 25).is_err());
        assert!(adapter.total_assets(&env, &asset).is_err());
    }

    #[test]
    fn test_cross_chain_adapter_submit_intent() {
        let adapter = TestCrossChainAdapter::new();
        let env = Env::default();
        let plan = Bytes::new(&env);

        let attempt_id = adapter.submit_intent(&env, plan).unwrap();
        assert_eq!(attempt_id, 1);
    }

    #[test]
    fn test_cross_chain_adapter_settle() {
        let receipt = SettlementReceipt::new(10, 20, 5000);
        let adapter = TestCrossChainAdapter::new().with_settlement(receipt.clone());
        let env = Env::default();

        let result = adapter.settle(&env, 10, 20).unwrap();
        assert_eq!(result, receipt);
    }

    #[test]
    fn test_cross_chain_adapter_total_assets() {
        let mut adapter = TestCrossChainAdapter::new();
        adapter.mock_total_assets = 2500;
        let env = Env::default();
        let asset = Address::generate(&env);

        assert_eq!(adapter.total_assets(&env, &asset).unwrap(), 2500);
    }
}

mod storage_tests {
    use crate::error::RuntimeError;
    use crate::storage::*;
    use rstest::{fixture, rstest};
    use soroban_sdk::{Address as SdkAddress, Env, Symbol};
    use templar_vault_kernel::VaultState;

    #[test]
    fn test_storage_version() {
        let v1 = StorageVersion::V1;
        assert_eq!(v1.number(), 1);
        assert!(v1.is_compatible());

        let current = StorageVersion::CURRENT;
        assert_eq!(current, StorageVersion::V1);
    }

    #[test]
    fn test_versioned_state_new() {
        let state = VaultState::default();
        let versioned = VersionedState::new(state);

        assert_eq!(versioned.version, StorageVersion::CURRENT);
    }

    #[test]
    fn test_memory_storage_empty() {
        let storage = MemoryStorage::new();
        assert!(!storage.is_initialized());
        assert!(storage.load_state().unwrap().is_none());
    }

    #[test]
    fn test_memory_storage_save_load() {
        let mut storage = MemoryStorage::new();
        let state = VersionedState::default();

        storage.save_state(&state).unwrap();
        assert!(storage.is_initialized());

        let loaded = storage.load_state().unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap(), state);
    }

    #[test]
    fn test_memory_storage_with_state() {
        let state = VersionedState::default();
        let storage = MemoryStorage::with_state(state.clone());

        assert!(storage.is_initialized());
        assert_eq!(storage.get_state(), Some(&state));
    }

    #[test]
    fn test_memory_storage_clear() {
        let state = VersionedState::default();
        let mut storage = MemoryStorage::with_state(state);

        storage.clear();
        assert!(!storage.is_initialized());
        assert!(storage.get_state().is_none());
    }

    #[test]
    fn test_memory_storage_address_book_roundtrip() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        let mut storage = MemoryStorage::new();
        let kernel_addr = [9u8; 32];
        let soroban_addr = SdkAddress::generate(&env);

        storage.save_address(&kernel_addr, &soroban_addr).unwrap();
        let loaded = storage.load_address(&kernel_addr).unwrap();
        assert_eq!(loaded, Some(soroban_addr));
    }

    #[test]
    fn test_storage_key_variants() {
        let key1 = StorageKey::VaultState;
        let key2 = StorageKey::Version;
        let key3 = StorageKey::PendingWithdrawal(42);
        let key4 = StorageKey::ShareBalance([0u8; 32]);
        let key5 = StorageKey::TotalSupply;

        assert_ne!(key1, key2);
        assert_ne!(key3, key4);
        assert_ne!(key4, key5);
    }

    #[test]
    fn test_soroban_storage_key_constants_are_distinct() {
        // All Symbol constants should be distinct from each other
        let keys: [Symbol; 9] = [
            SorobanStorageKey::StateBlob,
            SorobanStorageKey::PolicyLocks,
            SorobanStorageKey::PolicySupplyQueue,
            SorobanStorageKey::PolicyMarkets,
            SorobanStorageKey::PolicyPrincipals,
            SorobanStorageKey::PolicyCapGroups,
            SorobanStorageKey::Restrictions,
            SorobanStorageKey::Version,
            SorobanStorageKey::Paused,
        ];
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                assert_ne!(keys[i], keys[j], "keys at index {i} and {j} collide");
            }
        }
    }

    // Helper to create a registered contract for storage tests
    mod test_contract {
        use soroban_sdk::{contract, contractimpl, Env};

        #[contract]
        pub struct TestContract;

        #[contractimpl]
        impl TestContract {
            pub fn noop(_env: Env) {}
        }
    }

    #[fixture]
    fn contract_env() -> (Env, soroban_sdk::Address) {
        let env = Env::default();
        let contract_id = env.register(test_contract::TestContract, ());
        (env, contract_id)
    }

    #[rstest]
    fn test_soroban_storage_with_sdk_env(contract_env: (Env, soroban_sdk::Address)) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);

            // Fresh storage should not be initialized
            assert!(!storage.is_initialized());
            assert!(storage.get_version().is_none());
            assert!(storage.load_state_blob().is_none());

            // Save state
            let mut kernel = VaultState::default();
            kernel.total_assets = 10000;
            kernel.total_shares = 5000;
            kernel.idle_assets = 2000;
            kernel.external_assets = 8000;
            kernel.next_op_id = 1;
            let versioned = VersionedState::new(kernel);
            let mut storage_mut = SorobanStorage::new(&env);
            Storage::save_state(&mut storage_mut, &versioned).unwrap();

            // Now storage should be initialized
            assert!(storage.is_initialized());
            assert_eq!(
                storage.get_version(),
                Some(StorageVersion::CURRENT.number())
            );

            // Load and verify
            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded.state.total_assets, 10000);
            assert_eq!(loaded.state.total_shares, 5000);
            assert_eq!(loaded.state.idle_assets, 2000);
            assert_eq!(loaded.state.external_assets, 8000);
            assert_eq!(loaded.state.next_op_id, 1);
        });
    }

    #[rstest]
    fn test_soroban_storage_pause_state(contract_env: (Env, soroban_sdk::Address)) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);

            // Default is not paused
            assert!(!storage.is_paused());

            // Set paused
            storage.set_paused(true);
            assert!(storage.is_paused());

            // Unset paused
            storage.set_paused(false);
            assert!(!storage.is_paused());
        });
    }

    #[rstest]
    fn test_soroban_storage_roundtrip_op_state_and_queue(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        use alloc::collections::BTreeMap;
        use templar_vault_kernel::state::queue::{PendingWithdrawal, WithdrawQueue};
        use templar_vault_kernel::{OpState, WithdrawingState};

        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let mut state = VaultState::default();

            let owner = [1u8; 32];
            let receiver = [2u8; 32];
            state.op_state = OpState::Withdrawing(WithdrawingState {
                op_id: 7,
                index: 1,
                remaining: 500,
                collected: 200,
                receiver,
                owner,
                escrow_shares: 700,
            });

            let mut pending = BTreeMap::new();
            pending.insert(3, PendingWithdrawal::new(owner, receiver, 700, 800, 123));
            state.withdraw_queue = WithdrawQueue::with_state(pending, 3, 4);
            state.total_assets = 1000;
            state.total_shares = 900;
            state.idle_assets = 100;
            state.external_assets = 900;
            state.next_op_id = 8;

            let versioned = VersionedState::new(state.clone());
            storage.save_state(&versioned).unwrap();

            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded.state, state);
        });
    }

    #[rstest]
    fn test_soroban_storage_implements_storage_trait(contract_env: (Env, soroban_sdk::Address)) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);

            // Test Storage trait methods
            assert!(!Storage::is_initialized(&storage));
            assert!(storage.load_state().unwrap().is_none());

            // Save state via trait
            let versioned = VersionedState::default();
            storage.save_state(&versioned).unwrap();

            // Verify via trait
            assert!(Storage::is_initialized(&storage));
            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded.version, StorageVersion::CURRENT);
        });
    }

    #[rstest]
    fn test_storage_trait_get_version_fails_when_uninitialized(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let err = Storage::get_version(&storage).unwrap_err();
            assert_eq!(err, RuntimeError::storage_error("version not initialized"));
        });
    }

    #[rstest]
    fn test_soroban_storage_load_state_rejects_corrupted_blob(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            storage.save_state_blob(&alloc::vec![1, 2, 3, 4, 5]);

            let err = Storage::load_state(&storage).unwrap_err();
            assert_eq!(
                err,
                RuntimeError::storage_error("state blob deserialize failed")
            );
        });
    }

    #[rstest]
    fn test_soroban_storage_load_state_rejects_trailing_bytes(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let versioned = VersionedState::new(VaultState::default());
            let mut bytes = postcard::to_allocvec(&versioned).unwrap();
            bytes.push(0xff);
            storage.save_state_blob(&bytes);

            let err = Storage::load_state(&storage).unwrap_err();
            assert_eq!(
                err,
                RuntimeError::storage_error("state blob deserialize failed")
            );
        });
    }

    #[rstest]
    fn test_soroban_storage_load_state_rejects_missing_version_key(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let versioned = VersionedState::new(VaultState::default());
            let bytes = postcard::to_allocvec(&versioned).unwrap();
            storage.save_state_blob(&bytes);

            let err = Storage::load_state(&storage).unwrap_err();
            assert_eq!(err, RuntimeError::storage_error("state version missing"));
        });
    }

    #[rstest]
    fn test_soroban_storage_load_state_rejects_mismatched_version(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let versioned = VersionedState::new(VaultState::default());
            let bytes = postcard::to_allocvec(&versioned).unwrap();
            storage.save_state_blob(&bytes);
            storage.set_version(StorageVersion::new(2).number());

            let err = Storage::load_state(&storage).unwrap_err();
            assert_eq!(err, RuntimeError::storage_error("state version mismatch"));
        });
    }

    #[rstest]
    fn test_soroban_storage_load_state_rejects_incompatible_version(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let versioned =
                VersionedState::with_version(StorageVersion::new(2), VaultState::default());
            let bytes = postcard::to_allocvec(&versioned).unwrap();
            storage.save_state_blob(&bytes);
            storage.set_version(StorageVersion::new(2).number());

            let err = Storage::load_state(&storage).unwrap_err();
            assert_eq!(
                err,
                RuntimeError::storage_error("unsupported state version")
            );
        });
    }

    #[rstest]
    fn test_soroban_storage_loads_legacy_version_state(contract_env: (Env, soroban_sdk::Address)) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let mut state = VaultState::default();
            state.total_assets = 42;
            let legacy = VersionedState::with_version(StorageVersion::new(0), state.clone());
            storage.save_state(&legacy).unwrap();

            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded.version, StorageVersion::new(0));
            assert_eq!(loaded.state.total_assets, 42);
        });
    }

    #[rstest]
    fn test_soroban_storage_migrates_legacy_state_with_active_op(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        use alloc::collections::BTreeMap;
        use templar_vault_kernel::state::queue::{PendingWithdrawal, WithdrawQueue};
        use templar_vault_kernel::{OpState, WithdrawingState};

        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let mut state = VaultState::default();

            let owner = [1u8; 32];
            let receiver = [2u8; 32];
            state.op_state = OpState::Withdrawing(WithdrawingState {
                op_id: 11,
                index: 1,
                remaining: 500,
                collected: 200,
                receiver,
                owner,
                escrow_shares: 700,
            });

            let mut pending = BTreeMap::new();
            pending.insert(3, PendingWithdrawal::new(owner, receiver, 700, 800, 123));
            state.withdraw_queue = WithdrawQueue::with_state(pending, 3, 4);
            state.total_assets = 1000;
            state.total_shares = 900;
            state.idle_assets = 100;
            state.external_assets = 900;
            state.next_op_id = 12;

            let legacy = VersionedState::with_version(StorageVersion::new(0), state.clone());
            storage.save_state(&legacy).unwrap();

            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded.version, StorageVersion::new(0));
            assert_eq!(loaded.state, state);

            let migrated = VersionedState::new(loaded.state.clone());
            storage.save_state(&migrated).unwrap();
            let reloaded = storage.load_state().unwrap().unwrap();
            assert_eq!(reloaded.version, StorageVersion::CURRENT);
            assert_eq!(reloaded.state, state);
        });
    }

    #[rstest]
    #[case(StorageVersion::new(0), true)]
    #[case(StorageVersion::V1, true)]
    #[case(StorageVersion::new(2), false)]
    #[case(StorageVersion::new(u32::MAX), false)]
    fn test_storage_version_compatibility_cases(
        #[case] version: StorageVersion,
        #[case] expected: bool,
    ) {
        assert_eq!(version.is_compatible(), expected);
    }
}
