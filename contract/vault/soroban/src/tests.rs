// Kitchen-sink unit tests for Soroban vault runtime.

mod auth_tests {
    use crate::auth::{ActionKind, AuthError, SorobanAuth};
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address as SdkAddress, Env};
    use templar_curator_primitives::auth::{AuthPolicyClass, AuthResult};
    use templar_curator_primitives::rbac::Role;

    fn assert_missing_role(
        result: AuthResult<()>,
        action: ActionKind,
        policy_class: AuthPolicyClass,
    ) {
        assert!(matches!(
            result,
            Err(AuthError::MissingRole {
                action: actual_action,
                policy_class: actual_policy_class,
            }) if actual_action == action && actual_policy_class == policy_class
        ));
    }

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
        assert_missing_role(
            result,
            ActionKind::ExecuteWithdraw,
            AuthPolicyClass::Allocator,
        );
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
        assert_missing_role(result, ActionKind::Pause, AuthPolicyClass::Sentinel);

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
        assert_missing_role(
            result,
            ActionKind::BeginAllocating,
            AuthPolicyClass::Allocator,
        );
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
        assert_missing_role(
            result,
            ActionKind::AbortWithdrawing,
            AuthPolicyClass::AllocatorEmergency,
        );
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
        assert_missing_role(
            result,
            ActionKind::ManualReconcile,
            AuthPolicyClass::Curator,
        );

        assert!(auth.check_role(ActionKind::PolicyAdmin, &curator).is_ok());
        let result = auth.check_role(ActionKind::PolicyAdmin, &allocator);
        assert_missing_role(result, ActionKind::PolicyAdmin, AuthPolicyClass::Curator);
    }

    #[test]
    fn test_soroban_auth_paused_blocks_non_whitelisted_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);

        let mut auth =
            SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));
        auth.set_paused(true);

        let result = auth.check_role(ActionKind::BeginAllocating, &allocator);
        assert!(matches!(result, Err(AuthError::VaultPaused)));
        let result = auth.check_role(ActionKind::PolicyAdmin, &curator);
        assert!(matches!(result, Err(AuthError::VaultPaused)));
    }

    #[test]
    fn test_soroban_auth_paused_allows_whitelisted_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let sentinel = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);

        let mut auth = SorobanAuth::with_roles(
            &env,
            curator.clone(),
            Some(sentinel.clone()),
            Some(allocator),
        );
        auth.set_paused(true);

        assert!(auth.check_role(ActionKind::Pause, &sentinel).is_ok());
        assert!(auth
            .check_role(ActionKind::SetRestrictions, &sentinel)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::AbortAllocating, &sentinel)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::AbortWithdrawing, &sentinel)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::AbortRefreshing, &sentinel)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::ManualReconcile, &curator)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::EmergencyReset, &curator)
            .is_ok());
    }

    #[test]
    fn verify_and_authorize_requires_native_auth_even_for_public_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);
        let auth = SorobanAuth::new(&env, curator);
        let contract_id = env.register(crate::contract::SorobanVaultContract, ());

        assert!(auth.check_role(ActionKind::Deposit, &user).is_ok());

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.as_contract(&contract_id, || {
                auth.verify_and_authorize(ActionKind::Deposit, &user)
            })
        }));
        assert!(
            result.is_err(),
            "require_auth must not be bypassed by public policy"
        );
    }

    #[test]
    fn verify_and_authorize_records_native_auth_and_preserves_role_errors() {
        let env = Env::default();
        env.mock_all_auths();
        let curator = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);
        let auth = SorobanAuth::with_roles(&env, curator, None, Some(allocator.clone()));
        let contract_id = env.register(crate::contract::SorobanVaultContract, ());

        env.as_contract(&contract_id, || {
            assert!(auth
                .verify_and_authorize(ActionKind::BeginAllocating, &allocator)
                .is_ok());
        });
        assert_eq!(
            env.auths().len(),
            1,
            "successful verify path must call require_auth"
        );
        assert_eq!(env.auths()[0].0, allocator);

        let result = env.as_contract(&contract_id, || {
            auth.verify_and_authorize(ActionKind::BeginAllocating, &user)
        });
        assert_missing_role(
            result,
            ActionKind::BeginAllocating,
            AuthPolicyClass::Allocator,
        );
        assert_eq!(
            env.auths().len(),
            1,
            "role failure happens after require_auth succeeds"
        );
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
    use crate::contract::helpers::set_config_address;
    use crate::contract::*;
    use crate::convert::ledger_timestamp_ns;
    use crate::effects::{AddressRegistrar, EffectContext, EffectInterpreter, EffectResult};
    use crate::error::RuntimeError;
    use crate::storage::{SorobanStorage, Storage};
    use crate::test_utils::{begin_allocating, finish_allocating, MemoryStorage};
    use alloc::collections::BTreeMap;
    use alloc::string::{String as AllocString, ToString};
    use alloc::vec;
    use alloc::vec::Vec;
    use proptest::prelude::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address as SdkAddress, Bytes, Env};
    use templar_curator_primitives::policy::state::MarketConfig;
    use templar_curator_primitives::PolicyState;
    use templar_soroban_governance::SorobanVaultGovernanceContract;
    use templar_soroban_shared_types::{
        ExecuteWithdrawStatus, GovernanceCommand, VaultCommand, VaultCommandResult,
        GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS,
    };
    use templar_vault_kernel::effects::KernelEffect;
    use templar_vault_kernel::{
        FeeAccrualAnchor, FeesSpec, OpState, Restrictions, VaultState, WithdrawingState,
        MIN_WITHDRAWAL_ASSETS,
    };

    #[derive(Clone, Copy, Default)]
    struct TestPermissiveAuth;

    impl AuthAdapter for TestPermissiveAuth {
        fn authorize(
            &self,
            _action: ActionKind,
            _caller: templar_vault_kernel::Address,
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
        fn register_address(
            &mut self,
            _kernel_addr: templar_vault_kernel::Address,
            _soroban_addr: SdkAddress,
        ) {
        }

        fn has_address(&self, _kernel_addr: &templar_vault_kernel::Address) -> bool {
            true
        }
    }

    fn sdk_text(address: &SdkAddress) -> AllocString {
        AllocString::from_utf8(address.to_string().to_bytes().to_alloc_vec())
            .expect("valid address")
    }

    fn register_runtime_contracts(
        env: &Env,
        contract_id: &SdkAddress,
        admin: &SdkAddress,
    ) -> (SdkAddress, SdkAddress, SdkAddress) {
        let governance = env.register(
            SorobanVaultGovernanceContract,
            (admin, contract_id, &(0u64)),
        );
        let asset = env
            .register_stellar_asset_contract_v2(SdkAddress::generate(env))
            .address();
        let share = env
            .register_stellar_asset_contract_v2(contract_id.clone())
            .address();
        (governance, asset, share)
    }

    fn execute_command(
        env: &Env,
        command: &VaultCommand,
    ) -> Result<VaultCommandResult, crate::error::ContractError> {
        let payload = Bytes::from_slice(env, &command.encode());
        let result = SorobanVaultContract::execute(env.clone(), payload)?;
        VaultCommandResult::decode(&result.to_alloc_vec())
            .map_err(|_| crate::error::ContractError::InvalidInput)
    }

    fn execute_governance_command(
        env: &Env,
        caller: &SdkAddress,
        command: &GovernanceCommand,
    ) -> Result<(), crate::error::ContractError> {
        let payload = Bytes::from_slice(env, &command.encode());
        SorobanVaultContract::execute_governance(env.clone(), caller.clone(), payload)
    }

    #[derive(Clone, Debug, Default)]
    struct TrackingInterpreter {
        addresses: BTreeMap<templar_vault_kernel::Address, SdkAddress>,
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
        fn register_address(
            &mut self,
            kernel_addr: templar_vault_kernel::Address,
            soroban_addr: SdkAddress,
        ) {
            self.addresses.insert(kernel_addr, soroban_addr);
        }

        fn has_address(&self, kernel_addr: &templar_vault_kernel::Address) -> bool {
            self.addresses.contains_key(kernel_addr)
        }
    }

    fn test_config() -> ContractConfig {
        ContractConfig::new(
            templar_vault_kernel::Address([1u8; 32]),
            templar_vault_kernel::Address([9u8; 32]),
            vec![templar_vault_kernel::Address([3u8; 32])],
            templar_vault_kernel::Address([4u8; 32]),
            templar_vault_kernel::Address([5u8; 32]),
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
    fn test_initialize_onboards_default_storage_and_config() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let governance = SdkAddress::generate(&env);
        let asset_token = SdkAddress::generate(&env);
        let share_token = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                governance.clone(),
                asset_token.clone(),
                share_token.clone(),
                123,
                456,
            )
            .unwrap();

            let storage = SorobanStorage::new(&env);
            assert!(storage.is_initialized());
            assert_eq!(storage.load_state().unwrap(), Some(VaultState::default()));
            assert!(!storage.is_paused());
            assert_eq!(load_virtual_offsets(&env), (123, 456));
            assert_eq!(load_fees_spec(&env).unwrap(), FeesSpec::zero());
            assert_eq!(
                get_config_address(&env, &VaultDataKey::Curator).unwrap(),
                curator
            );
            assert_eq!(
                get_config_address(&env, &VaultDataKey::Governance).unwrap(),
                governance
            );
            assert_eq!(
                get_config_address(&env, &VaultDataKey::AssetToken).unwrap(),
                asset_token
            );
            assert_eq!(
                get_config_address(&env, &VaultDataKey::ShareToken).unwrap(),
                share_token
            );
        });
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

        assert_eq!(derived, templar_vault_kernel::Address(expected));
        assert_ne!(derived, templar_vault_kernel::Address(raw_hash));
    }

    #[test]
    fn address_from_alloc_string_rejects_invalid_strkey() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        let invalid = AllocString::from("not-a-stellar-address");
        let valid = SdkAddress::generate(&env).to_string().to_string();
        let mut bad_checksum = valid.clone();
        bad_checksum.pop();
        bad_checksum.push(if valid.ends_with('A') { 'B' } else { 'A' });

        assert!(address_from_alloc_string(&env, &valid).is_ok());
        assert_eq!(
            address_from_alloc_string(&env, &invalid),
            Err(crate::error::ContractError::InvalidInput)
        );
        assert_eq!(
            address_from_alloc_string(&env, &bad_checksum),
            Err(crate::error::ContractError::InvalidInput)
        );
    }

    #[test]
    fn test_deposit_first() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([10u8; 32]);

        let result = vault.deposit(caller, receiver, 1000, 0, 100).unwrap();

        assert_eq!(result.shares_minted, 1000);
        assert_eq!(result.total_shares, 1000);
        assert_eq!(result.total_assets, 1000);
    }

    #[test]
    fn test_deposit_subsequent() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([10u8; 32]);

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
        let caller = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([10u8; 32]);

        let result = vault.deposit(caller, receiver, 0, 0, 100);

        assert!(result.is_err());
    }

    #[test]
    fn test_deposit_slippage_fails() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([10u8; 32]);

        // Deposit with min_shares_out higher than actual
        let result = vault.deposit(caller, receiver, 1000, 2000, 100);

        assert!(result.is_err());
    }

    #[test]
    fn test_begin_allocating() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]); // allocator

        let state = vault.state_mut().unwrap();
        state.idle_assets = 2_000;
        state.total_assets = 2_000;

        let op_id = begin_allocating(&mut vault, caller, vec![(0, 500), (1, 500)], 1000).unwrap();

        assert_eq!(op_id, 0);
        assert!(vault.state().unwrap().op_state.is_allocating());
    }

    #[test]
    fn test_begin_allocating_helper_matches_production_begin_state() {
        let caller = templar_vault_kernel::Address([3u8; 32]); // allocator
        let owner = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([2u8; 32]);
        let deposit_amount = 2_000u128;
        let supply_amount = 500u128;
        let plan = [templar_vault_kernel::AllocationPlanEntry::new(
            0,
            supply_amount,
        )];

        let mut helper_vault = create_test_vault();
        helper_vault
            .deposit(owner, receiver, deposit_amount, 0, 100)
            .unwrap();
        let helper_op_id =
            begin_allocating(&mut helper_vault, caller, vec![(0, supply_amount)], 1_000).unwrap();

        let mut production_vault = create_test_vault();
        production_vault
            .deposit(owner, receiver, deposit_amount, 0, 100)
            .unwrap();
        let production_op_id = production_vault
            .begin_allocation_internal(caller, &plan, 1_000)
            .unwrap();

        let helper_state = helper_vault.state().unwrap();
        let production_state = production_vault.state().unwrap();
        assert_eq!(helper_op_id, production_op_id);
        assert_eq!(helper_state.idle_assets, production_state.idle_assets);
        assert_eq!(
            helper_state.external_assets,
            production_state.external_assets
        );
        assert_eq!(helper_state.total_assets, production_state.total_assets);
        assert_eq!(helper_state.op_state, production_state.op_state);
    }

    #[test]
    fn test_direct_supply_allocation_covers_production_completion_flow() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]); // allocator
        let owner = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([2u8; 32]);

        vault.deposit(owner, receiver, 2_000, 0, 100).unwrap();
        vault
            .policy_state_mut()
            .set_market_config(0, MarketConfig::new(true, u128::MAX, None))
            .unwrap();
        vault.policy_state_mut().set_principal(0, 0).unwrap();

        let result = vault
            .allocate(
                caller,
                &AllocationDelta::Supply(Delta {
                    market: 0,
                    amount: 500,
                }),
            )
            .unwrap();
        let state = vault.state().unwrap();

        assert_eq!(result.new_external_assets, 500);
        assert_eq!(state.idle_assets, 1_500);
        assert_eq!(state.external_assets, 500);
        assert_eq!(state.total_assets, 2_000);
        assert!(state.op_state.is_idle());
    }

    #[test]
    fn test_finish_allocating() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]); // allocator

        let state = vault.state_mut().unwrap();
        state.idle_assets = 2_000;
        state.total_assets = 2_000;

        let op_id = begin_allocating(&mut vault, caller, vec![(0, 500)], 1000).unwrap();

        let result = finish_allocating(&mut vault, caller, op_id).unwrap();

        assert_eq!(result.op_id, op_id);
        assert!(vault.state().unwrap().op_state.is_idle());
    }

    #[test]
    fn test_complete_supply_allocation_rejects_observed_assets_above_supplied_step() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]);

        {
            let policy = vault.policy_state_mut();
            policy
                .set_market_config(0, MarketConfig::new(true, 10_000, None))
                .unwrap();
            policy.set_principal(0, 1_000).unwrap();
        }
        {
            let state = vault.state_mut().unwrap();
            state.idle_assets = 1_000;
            state.external_assets = 1_000;
            state.total_assets = 2_000;
        }
        let op_id = begin_allocating(&mut vault, caller, vec![(0, 500)], 1_000).unwrap();

        let error = vault
            .complete_supply_allocation(caller, 0, 1_501, op_id, 1_000)
            .expect_err("supply callback must not over-report the active step");

        assert_eq!(error, RuntimeError::InvalidState);
        assert!(vault.state().unwrap().op_state.is_allocating());
        assert_eq!(vault.state().unwrap().external_assets, 1_000);
        assert_eq!(vault.policy_state().principal_for(0), Some(1_000));
    }

    #[test]
    fn test_sync_external_assets_requires_active_allocation_op_id() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]); // allocator

        {
            let state = vault.state_mut().unwrap();
            state.idle_assets = 4_000;
            state.external_assets = 6_000;
            state.total_assets = 10_000;
        }

        let op_id = vault
            .begin_allocation_withdraw_internal(caller, 0, 1_000)
            .unwrap();

        let synced_external = vault
            .sync_external_assets(caller, op_id, 4_500, 1_000)
            .unwrap();

        assert_eq!(synced_external, 4_500);
        assert!(vault.state().unwrap().op_state.is_allocating());
        assert_eq!(vault.state().unwrap().external_assets, 4_500);
        assert_eq!(vault.state().unwrap().idle_assets, 4_000);
        assert_eq!(vault.state().unwrap().total_assets, 8_500);

        let stale_sync = vault.sync_external_assets(caller, op_id + 1, 4_000, 1_000);
        assert!(stale_sync.is_err());
    }

    #[test]
    fn test_begin_refreshing() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]); // allocator

        let op_id = vault.begin_refreshing(caller, vec![0, 1], 1000).unwrap();

        assert_eq!(op_id, 0);
        assert!(vault.state().unwrap().op_state.is_refreshing());
    }

    #[test]
    fn test_finish_refreshing_reports_markets_refreshed() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]); // allocator

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

        let result = vault.finish_refreshing(caller, op_id, 1500).unwrap();

        assert_eq!(result.markets_refreshed, expected);
        assert!(vault.state().unwrap().op_state.is_idle());
    }

    #[test]
    fn test_complete_refresh_with_positions_rejects_targets_outside_active_plan() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]);

        let op_id = vault
            .begin_refreshing(caller, vec![0, 1], 1_500)
            .expect("should start refresh");

        let error = vault
            .complete_refresh_with_positions(caller, &[(0, 100), (2, 300)], op_id, 1_500)
            .expect_err("extra targets must be rejected");

        assert_eq!(error, RuntimeError::InvalidInput);
        assert!(vault.state().unwrap().op_state.is_refreshing());
    }

    #[test]
    fn test_complete_refresh_rejects_cumulative_adapter_inflation() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]);

        {
            let policy = vault.policy_state_mut();
            policy
                .set_market_config(0, MarketConfig::new(true, 1_998, None))
                .unwrap();
            policy.set_principal(0, 999).unwrap();
        }
        {
            let state = vault.state_mut().unwrap();
            state.idle_assets = 1_000;
            state.external_assets = 999;
            state.total_assets = 1_999;
        }
        let op_id = vault.begin_refreshing(caller, vec![0], 1_500).unwrap();

        let error = vault
            .complete_refresh_with_positions(caller, &[(0, 2_997)], op_id, 1_500)
            .expect_err("refresh must not accept cumulative adapter inflation");

        assert_eq!(error, RuntimeError::InvalidState);
        assert!(vault.state().unwrap().op_state.is_refreshing());
        assert_eq!(vault.state().unwrap().external_assets, 999);
        assert_eq!(vault.policy_state().principal_for(0), Some(999));
    }

    #[test]
    fn test_complete_refresh_allows_adapter_reported_decrease_within_cap() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]);

        {
            let policy = vault.policy_state_mut();
            policy
                .set_market_config(0, MarketConfig::new(true, 10_000, None))
                .unwrap();
            policy.set_principal(0, 10_000).unwrap();
        }
        {
            let state = vault.state_mut().unwrap();
            state.idle_assets = 1_000;
            state.external_assets = 10_000;
            state.total_assets = 11_000;
        }
        let op_id = vault.begin_refreshing(caller, vec![0], 1_500).unwrap();

        let result = vault
            .complete_refresh_with_positions(caller, &[(0, 0)], op_id, 1_500)
            .expect("refresh may report a legitimate market decrease");

        assert_eq!(result.markets_refreshed, 1);
        assert!(vault.state().unwrap().op_state.is_idle());
        assert_eq!(vault.state().unwrap().external_assets, 0);
        assert_eq!(vault.state().unwrap().total_assets, 1_000);
        assert_eq!(vault.policy_state().principal_for(0), Some(0));
    }

    #[test]
    fn test_complete_refresh_with_positions_accounts_for_external_growth() {
        let mut vault = create_test_vault();
        let caller = templar_vault_kernel::Address([3u8; 32]);
        let owner = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([2u8; 32]);

        let deposit_amount = 1_000_000u128;
        let supply_amount = 400_000u128;
        let growth = 25_000u128;
        vault
            .deposit(owner, receiver, deposit_amount, 0, 100)
            .unwrap();
        vault
            .policy_state_mut()
            .set_market_config(0, MarketConfig::new(true, u128::MAX, None))
            .unwrap();
        vault.policy_state_mut().set_principal(0, 0).unwrap();
        vault
            .allocate(
                caller,
                &AllocationDelta::Supply(Delta {
                    market: 0,
                    amount: supply_amount,
                }),
            )
            .unwrap();

        let total_before = vault.state().unwrap().total_assets;
        let grown_external = supply_amount + growth;
        let op_id = vault.begin_refreshing(caller, vec![0], 1_500).unwrap();
        let result = vault
            .complete_refresh_with_positions(caller, &[(0, grown_external)], op_id, 1_600)
            .unwrap();
        let state = vault.state().unwrap();

        assert_eq!(result.new_external_assets, grown_external);
        assert_eq!(state.external_assets, grown_external);
        assert_eq!(state.total_assets, total_before + growth);
        assert_eq!(
            state.total_assets,
            state.idle_assets + state.external_assets
        );
        assert_eq!(state.total_shares, deposit_amount);
        assert!(state.op_state.is_idle());
    }

    #[test]
    fn test_execute_withdraw_respects_min_withdrawal_assets() {
        let mut vault = create_test_vault();
        let allocator = templar_vault_kernel::Address([3u8; 32]);
        let owner = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([2u8; 32]);

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

        let error = vault
            .execute_withdraw(allocator, exec_time)
            .expect_err("low-liquidity withdrawal should not start");

        assert_eq!(error, RuntimeError::KernelError);
        let state = vault.state().unwrap();
        assert!(state.op_state.is_idle());
        let (head_id_after, head_after) = state
            .withdraw_queue
            .head()
            .expect("withdrawal still queued");
        assert_eq!(head_id_after, head_id);
        assert_eq!(head_after.escrow_shares, head_escrow_before);
        assert_eq!(head_after.expected_assets, head_expected_before);
        assert_eq!(state.idle_assets, MIN_WITHDRAWAL_ASSETS.saturating_sub(1));
        assert_eq!(
            state.total_assets,
            state.idle_assets.saturating_add(state.external_assets)
        );
        assert_eq!(state.total_shares, deposit_amount);
        assert_eq!(head_id, 0);
        assert_eq!(head_expected_before, deposit_amount);
    }

    #[test]
    fn test_abort_withdrawing_recovers_low_liquidity_stuck_state() {
        let mut vault = create_test_vault();
        let allocator = templar_vault_kernel::Address([3u8; 32]);
        let owner = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([2u8; 32]);

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

        {
            let state = vault.state_mut().unwrap();
            state.idle_assets = MIN_WITHDRAWAL_ASSETS.saturating_sub(1);
            state.total_assets = state.idle_assets.saturating_add(state.external_assets);
            let (request_id, owner, receiver, escrow_shares, expected_assets) = {
                let (request_id, head) = state.withdraw_queue.head().expect("withdrawal queued");
                (
                    request_id,
                    head.owner,
                    head.receiver,
                    head.escrow_shares,
                    head.expected_assets,
                )
            };
            let op_id = state.allocate_op_id();
            state.op_state = OpState::Withdrawing(WithdrawingState {
                op_id,
                request_id,
                index: 0,
                remaining: expected_assets,
                collected: 0,
                owner,
                receiver,
                escrow_shares,
            });
        }

        let op_id = vault.state().unwrap().op_state.op_id().unwrap();
        let recovery_summary = vault
            .abort_withdrawing(allocator, op_id, exec_time.saturating_add(1))
            .unwrap();

        let state = vault.state().unwrap();
        assert!(state.op_state.is_idle());
        assert!(state.withdraw_queue.is_empty());
        assert_eq!(state.idle_assets, MIN_WITHDRAWAL_ASSETS.saturating_sub(1));
        assert_eq!(
            state.total_assets,
            state.idle_assets.saturating_add(state.external_assets)
        );
        assert_eq!(state.total_shares, deposit_amount);
        assert_eq!(recovery_summary.assets_transferred, 0);
        assert_eq!(recovery_summary.shares_burned, 0);
        assert_eq!(recovery_summary.shares_transferred, deposit_amount);
        assert_eq!(recovery_summary.events_emitted, 1);
    }

    proptest! {
        #[test]
        fn prop_low_liquidity_execute_refuses_and_stale_withdrawing_has_abort_recovery(
            deposit_multiple in 2u128..=32,
            extra_assets in 0u128..=MIN_WITHDRAWAL_ASSETS,
            low_idle in 0u128..MIN_WITHDRAWAL_ASSETS,
            retry_count in 0usize..=3,
        ) {
            let mut vault = create_test_vault();
            let allocator = templar_vault_kernel::Address([3u8; 32]);
            let owner = templar_vault_kernel::Address([1u8; 32]);
            let receiver = templar_vault_kernel::Address([2u8; 32]);

            let deposit_amount = MIN_WITHDRAWAL_ASSETS
                .saturating_mul(deposit_multiple)
                .saturating_add(extra_assets);
            let request_time: u64 = 200;
            let exec_time = request_time
                .saturating_add(templar_vault_kernel::DEFAULT_COOLDOWN_NS)
                .saturating_add(1);

            vault
                .deposit(owner, receiver, deposit_amount, 0, request_time)
                .expect("deposit should succeed");
            vault
                .request_withdraw(owner, receiver, deposit_amount, 0, request_time)
                .expect("withdraw request should succeed");

            {
                let state = vault.state_mut().expect("state is loaded");
                state.idle_assets = low_idle;
                state.total_assets = state.idle_assets.saturating_add(state.external_assets);
            }

            let error = vault
                .execute_withdraw(allocator, exec_time)
                .expect_err("low-liquidity execution should be refused");
            prop_assert_eq!(error, RuntimeError::KernelError);
            prop_assert!(vault.state().expect("state is loaded").op_state.is_idle());
            prop_assert!(!vault.state().expect("state is loaded").withdraw_queue.is_empty());

            {
                let state = vault.state_mut().expect("state is loaded");
                let (request_id, owner, receiver, escrow_shares, expected_assets) = {
                    let (request_id, head) = state.withdraw_queue.head().expect("withdrawal queued");
                    (
                        request_id,
                        head.owner,
                        head.receiver,
                        head.escrow_shares,
                        head.expected_assets,
                    )
                };
                let op_id = state.allocate_op_id();
                state.op_state = OpState::Withdrawing(WithdrawingState {
                    op_id,
                    request_id,
                    index: 0,
                    remaining: expected_assets,
                    collected: 0,
                    owner,
                    receiver,
                    escrow_shares,
                });
            }

            for offset in 0..retry_count {
                let retry_error = vault
                    .execute_withdraw(allocator, exec_time.saturating_add(offset as u64 + 1))
                    .expect_err("stale low-liquidity withdrawal should remain blocked");
                prop_assert_eq!(retry_error, RuntimeError::KernelError);
                prop_assert!(vault.state().expect("state is loaded").op_state.is_withdrawing());
            }

            let op_id = vault
                .state()
                .expect("state is loaded")
                .op_state
                .as_withdrawing()
                .expect("withdrawal should remain active")
                .op_id;
            let recovery_summary = vault
                .abort_withdrawing(allocator, op_id, exec_time.saturating_add(10))
                .expect("abort_withdrawing should recover stuck state");

            let state = vault.state().expect("state is loaded");
            prop_assert!(state.op_state.is_idle());
            prop_assert!(state.withdraw_queue.is_empty());
            prop_assert_eq!(state.idle_assets, low_idle);
            prop_assert_eq!(
                state.total_assets,
                state.idle_assets.saturating_add(state.external_assets)
            );
            prop_assert_eq!(state.total_shares, deposit_amount);
            prop_assert_eq!(recovery_summary.assets_transferred, 0);
            prop_assert_eq!(recovery_summary.shares_burned, 0);
            prop_assert_eq!(recovery_summary.shares_transferred, deposit_amount);
            prop_assert_eq!(recovery_summary.events_emitted, 1);
        }
    }

    #[test]
    fn test_execute_withdraw_insufficient_idle_partially_settles() {
        let mut vault = create_test_vault();
        let allocator = templar_vault_kernel::Address([3u8; 32]);
        let owner = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([2u8; 32]);

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

        let (_head_id, head_escrow_before, head_expected_before) = {
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

        assert_eq!(
            summary.assets_transferred,
            MIN_WITHDRAWAL_ASSETS.saturating_add(1)
        );
        assert_eq!(
            summary.shares_burned,
            MIN_WITHDRAWAL_ASSETS.saturating_add(1)
        );
        let state = vault.state().unwrap();
        assert!(state.op_state.is_idle());
        assert!(state.withdraw_queue.is_empty());
        assert_eq!(state.idle_assets, 0);
        assert_eq!(state.total_assets, state.external_assets);
        assert_eq!(state.total_shares, deposit_amount - summary.shares_burned);
        assert_eq!(
            summary.shares_transferred,
            head_escrow_before - summary.shares_burned
        );
        assert_eq!(head_expected_before, deposit_amount);
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

            let (owner_kernel, receiver_kernel) = vault.map_pair(&env, &owner, &receiver).unwrap();
            vault
                .deposit(owner_kernel, receiver_kernel, assets, 0, now_ns)
                .unwrap();
            vault
                .request_withdraw(owner_kernel, receiver_kernel, assets, 0, now_ns)
                .unwrap();

            let storage = vault.storage.clone();

            let mut next_vault = CuratorVault::new(
                ContractConfig::new(
                    curator_kernel,
                    vault_kernel,
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
            let executor_kernel = next_vault.map_caller(&env, &executor).unwrap();
            let summary = next_vault
                .execute_withdraw(executor_kernel, exec_time)
                .unwrap();

            assert!(summary.assets_transferred > 0);
            assert!(next_vault.interpreter.has_address(&receiver_kernel));
        });
    }

    #[test]
    fn test_contract_config() {
        let config = test_config();

        assert!(config.is_curator(&templar_vault_kernel::Address([1u8; 32])));
        assert!(!config.is_curator(&templar_vault_kernel::Address([2u8; 32])));

        assert!(config.is_allocator(&templar_vault_kernel::Address([3u8; 32])));
        assert!(!config.is_allocator(&templar_vault_kernel::Address([1u8; 32])));

        assert!(config.is_privileged(&templar_vault_kernel::Address([1u8; 32]))); // curator
        assert!(config.is_privileged(&templar_vault_kernel::Address([3u8; 32]))); // allocator
        assert_eq!(config.virtual_shares, 0);
        assert_eq!(config.virtual_assets, 0);
    }

    #[test]
    fn test_contract_config_with_virtual_offsets() {
        let config = test_config().with_virtual_offsets(17, 29);

        assert_eq!(config.virtual_shares, 17);
        assert_eq!(config.virtual_assets, 29);
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
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                governance,
                asset,
                share,
                0,
                0,
            )
            .unwrap();
        });

        let fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, templar_vault_kernel::Address([1u8; 32])),
            FeeSlot::new(Wad::one() / 20, templar_vault_kernel::Address([2u8; 32])),
            None,
        );

        env.as_contract(&contract_id, || {
            crate::contract::store_fees_spec(&env, &fees).expect("store fees spec");
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
    fn test_rejects_fees_spec_trailing_bytes() {
        let env = Env::default();
        let contract_id = env.register(SorobanVaultContract, ());

        env.as_contract(&contract_id, || {
            let bytes = vec![0u8; 113];
            env.storage()
                .instance()
                .set(&VaultDataKey::FeesSpec, &Bytes::from_slice(&env, &bytes));

            assert!(crate::contract::load_fees_spec(&env).is_err());
        });
    }

    #[test]
    fn test_loads_virtual_offsets_from_storage() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

        let contract_id = env.register(SorobanVaultContract, ());
        let curator = soroban_sdk::Address::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                governance,
                asset,
                share,
                17,
                29,
            )
            .unwrap();

            let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
                assert_eq!(vault.config.virtual_shares, 17);
                assert_eq!(vault.config.virtual_assets, 29);
                Ok(())
            };
            with_contract_vault(&env, &mut call).unwrap();
        });
    }

    #[test]
    fn test_set_virtual_offsets_updates_contract_storage() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

        let contract_id = env.register(SorobanVaultContract, ());
        let curator = soroban_sdk::Address::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            execute_governance_command(
                &env,
                &governance,
                &GovernanceCommand::SetGovernanceConfig {
                    kind: GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS,
                    primary: None,
                    many: None,
                    value_a: Some(101),
                    value_b: Some(202),
                },
            )
            .unwrap();

            assert_eq!(
                env.storage().instance().get(&VaultDataKey::VirtualShares),
                Some(101u128)
            );
            assert_eq!(
                env.storage().instance().get(&VaultDataKey::VirtualAssets),
                Some(202u128)
            );
        });
    }

    #[test]
    fn test_rejects_virtual_offset_updates_after_capitalization() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

        let contract_id = env.register(SorobanVaultContract, ());
        let curator = soroban_sdk::Address::generate(&env);
        let governance = soroban_sdk::Address::generate(&env);
        let asset = soroban_sdk::Address::generate(&env);
        let share = soroban_sdk::Address::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                11,
                7,
            )
            .unwrap();

            let mut storage = SorobanStorage::new(&env);
            storage
                .save_state(&VaultState {
                    total_assets: 1_500,
                    total_shares: 1_000,
                    idle_assets: 1_500,
                    ..Default::default()
                })
                .expect("save capitalized state");

            let err = execute_governance_command(
                &env,
                &governance,
                &GovernanceCommand::SetGovernanceConfig {
                    kind: GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS,
                    primary: None,
                    many: None,
                    value_a: Some(101),
                    value_b: Some(202),
                },
            )
            .expect_err("capitalized vault must not accept virtual-offset changes");

            assert_eq!(err, crate::error::ContractError::InvalidState);
            assert_eq!(
                env.storage().instance().get(&VaultDataKey::VirtualShares),
                Some(11u128)
            );
            assert_eq!(
                env.storage().instance().get(&VaultDataKey::VirtualAssets),
                Some(7u128)
            );
        });
    }

    #[test]
    fn test_rejects_virtual_offset_updates_after_first_deposit_lock() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

        let contract_id = env.register(SorobanVaultContract, ());
        let curator = soroban_sdk::Address::generate(&env);
        let governance = soroban_sdk::Address::generate(&env);
        let asset = soroban_sdk::Address::generate(&env);
        let share = soroban_sdk::Address::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                11,
                7,
            )
            .unwrap();
            env.storage()
                .instance()
                .set(&VaultDataKey::VirtualOffsetsLocked, &true);
            SorobanStorage::new(&env)
                .save_state(&VaultState::default())
                .expect("save fully unwound state");

            let err = execute_governance_command(
                &env,
                &governance,
                &GovernanceCommand::SetGovernanceConfig {
                    kind: GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS,
                    primary: None,
                    many: None,
                    value_a: Some(101),
                    value_b: Some(202),
                },
            )
            .expect_err("first-deposit lock must survive zero accounting state");

            assert_eq!(err, crate::error::ContractError::InvalidState);
            assert_eq!(
                env.storage().instance().get(&VaultDataKey::VirtualShares),
                Some(11u128)
            );
            assert_eq!(
                env.storage().instance().get(&VaultDataKey::VirtualAssets),
                Some(7u128)
            );
        });
    }

    #[test]
    fn test_deposit_uses_configured_virtual_offsets() {
        let mut vault = CuratorVault::new(
            test_config().with_virtual_offsets(11, 7),
            MemoryStorage::new(),
            TestPermissiveAuth,
            MockInterpreter::new(),
        );
        vault.load_state().unwrap();

        let caller = templar_vault_kernel::Address([1u8; 32]);
        let receiver = templar_vault_kernel::Address([10u8; 32]);
        let result = vault.deposit(caller, receiver, 1_000, 0, 100).unwrap();

        assert_eq!(result.shares_minted, 1_571);
        assert_eq!(result.total_shares, 1_571);
        assert_eq!(result.total_assets, 1_000);
    }

    struct VaultProxy<'a> {
        env: &'a Env,
    }

    impl<'a> VaultProxy<'a> {
        const fn new(env: &'a Env) -> Self {
            Self { env }
        }

        fn initialize(
            &self,
            curator: soroban_sdk::Address,
            governance: soroban_sdk::Address,
            asset: soroban_sdk::Address,
            share: soroban_sdk::Address,
        ) -> Result<(), crate::error::ContractError> {
            SorobanVaultContract::initialize(
                self.env.clone(),
                curator,
                governance,
                asset,
                share,
                0,
                0,
            )
        }

        fn withdraw(
            &self,
            receiver: &soroban_sdk::Address,
            owner: &soroban_sdk::Address,
            operator: &soroban_sdk::Address,
            assets: i128,
        ) -> Result<i128, RuntimeError> {
            let mut result = None;
            let mut call = |vault: &mut ContractVault<'_>| -> Result<(), RuntimeError> {
                result = Some(vault.atomic_withdraw(
                    self.env,
                    assets,
                    i128::MAX,
                    receiver.clone(),
                    owner.clone(),
                    operator.clone(),
                )?);
                Ok(())
            };
            with_contract_vault(self.env, &mut call).map(|()| result.unwrap_or(0))
        }
    }

    #[test]
    fn test_atomic_withdraw_refreshes_fees() {
        use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
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
        let asset = soroban_sdk::Address::generate(&env);
        let share = soroban_sdk::Address::generate(&env);
        let owner = soroban_sdk::Address::generate(&env);
        let receiver = soroban_sdk::Address::generate(&env);
        let operator = owner.clone();
        let mgmt_recipient = soroban_sdk::Address::generate(&env);
        let perf_recipient = soroban_sdk::Address::generate(&env);

        env.as_contract(&contract_id, || {
            let config = ContractConfig::new(
                kernel_address_from_sdk(&env, &curator),
                kernel_address_from_sdk(&env, &contract_id),
                vec![],
                kernel_address_from_sdk(&env, &asset),
                kernel_address_from_sdk(&env, &share),
            )
            .with_fees(FeesSpec::new(
                FeeSlot::new(
                    Wad::one() / 10,
                    kernel_address_from_sdk(&env, &perf_recipient),
                ),
                FeeSlot::new(
                    Wad::one() / 10,
                    kernel_address_from_sdk(&env, &mgmt_recipient),
                ),
                None,
            ));
            let mut storage = MemoryStorage::with_state(VaultState {
                total_assets: 1_500,
                total_shares: 1_000,
                idle_assets: 1_500,
                fee_anchor: FeeAccrualAnchor::new(1_000, templar_vault_kernel::TimestampNs(0)),
                ..Default::default()
            });
            storage
                .save_address(
                    &kernel_address_from_sdk(&env, &mgmt_recipient),
                    &mgmt_recipient,
                )
                .expect("save management recipient address");
            storage
                .save_address(
                    &kernel_address_from_sdk(&env, &perf_recipient),
                    &perf_recipient,
                )
                .expect("save performance recipient address");
            let mut vault =
                CuratorVault::new(config, storage, TestPermissiveAuth, MockInterpreter::new());
            vault.load_state().expect("load state");

            let burned = vault
                .atomic_withdraw(
                    &env,
                    500,
                    i128::MAX,
                    receiver.clone(),
                    owner.clone(),
                    operator,
                )
                .expect("withdraw should succeed");
            assert!(burned > 0);

            let minted_effects: Vec<_> = vault
                .interpreter
                .effects
                .iter()
                .filter_map(|effect| match effect {
                    KernelEffect::MintShares { owner, shares } => Some((*owner, *shares)),
                    _ => None,
                })
                .collect();
            assert_eq!(minted_effects.len(), 1);
            assert!(minted_effects.iter().any(|(owner, shares)| {
                *owner == kernel_address_from_sdk(&env, &perf_recipient) && *shares > 0
            }));
            assert!(!minted_effects
                .iter()
                .any(|(owner, _)| { *owner == kernel_address_from_sdk(&env, &mgmt_recipient) }));

            let burned_effect = vault.interpreter.effects.iter().any(|effect| {
                matches!(
                    effect,
                    KernelEffect::BurnShares { owner: effect_owner, shares }
                        if *effect_owner == kernel_address_from_sdk(&env, &owner) && *shares > 0
                )
            });
            assert!(burned_effect);

            let state = vault.state().expect("state loaded");
            assert_eq!(state.fee_anchor.total_assets, 1_500);
            assert_eq!(
                state.fee_anchor.timestamp_ns,
                templar_vault_kernel::TimestampNs(ledger_timestamp_ns(&env).expect("timestamp"))
            );
        });
    }

    #[test]
    fn test_proxy_view_uses_fee_aware_kernel_conversions_for_high_values() {
        use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
        use soroban_sdk::token::StellarAssetClient;
        use templar_vault_kernel::fee::FeeSlot;
        use templar_vault_kernel::math::wad::Wad;
        use templar_vault_kernel::{
            convert_to_assets_ceil, convert_to_shares_ceil, Number, VaultConfig,
        };

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set(LedgerInfo {
            timestamp: 1_000,
            protocol_version: 25,
            ..Default::default()
        });

        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let governance = SdkAddress::generate(&env);
        let asset_admin = SdkAddress::generate(&env);
        let asset_sac = env.register_stellar_asset_contract_v2(asset_admin.clone());
        let asset = asset_sac.address();
        let asset_admin_client = StellarAssetClient::new(&env, &asset);
        let share_sac = env.register_stellar_asset_contract_v2(contract_id.clone());
        let share = share_sac.address();
        let owner = SdkAddress::generate(&env);
        let proxy = VaultProxy::new(&env);

        env.as_contract(&contract_id, || {
            proxy
                .initialize(curator, governance, asset.clone(), share.clone())
                .unwrap();
            set_config_address(&env, &VaultDataKey::AssetToken, &asset);
            set_config_address(&env, &VaultDataKey::ShareToken, &share);

            let fees = FeesSpec::new(
                FeeSlot::new(Wad::one() / 8, templar_vault_kernel::Address([8u8; 32])),
                FeeSlot::new(Wad::one() / 16, templar_vault_kernel::Address([9u8; 32])),
                None,
            );
            store_fees_spec(&env, &fees).unwrap();
            let state = VaultState {
                // High-value balances keep this parity test on the Soroban-specific large-number path.
                total_assets: 9_000_000_000_000_000_000,
                total_shares: 6_000_000_000_000_000_000,
                idle_assets: 7_000_000_000_000_000_000,
                external_assets: 2_000_000_000_000_000_000,
                fee_anchor: FeeAccrualAnchor::new(
                    8_000_000_000_000_000_000,
                    templar_vault_kernel::TimestampNs(0),
                ),
                ..Default::default()
            };
            let mut storage = SorobanStorage::new(&env);
            storage.save_state(&state).unwrap();
            asset_admin_client.mint(&contract_id, &(state.idle_assets as i128));

            let config = VaultConfig {
                fees,
                min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
                withdrawal_cooldown_ns: templar_vault_kernel::DEFAULT_COOLDOWN_NS,
                max_pending_withdrawals: templar_vault_kernel::MAX_PENDING as u32,
                paused: false,
                virtual_shares: 0,
                virtual_assets: 0,
            };
            let expected_state = {
                let mut expected = state;
                let now_ns = env.ledger().timestamp() * 1_000_000_000;
                let current_assets = expected.total_assets;
                let fee_assets_base = templar_vault_kernel::total_assets_for_fee_accrual(
                    current_assets,
                    expected.fee_anchor.total_assets,
                    expected.fee_anchor.timestamp_ns.as_u64(),
                    now_ns,
                    config.fees.max_total_assets_growth_rate,
                );
                let management_shares = templar_vault_kernel::compute_management_fee_shares(
                    fee_assets_base,
                    current_assets,
                    expected.total_shares,
                    config.fees.management.fee_wad,
                    expected.fee_anchor.timestamp_ns.as_u64(),
                    now_ns,
                );
                let supply_after_management =
                    Number::from(expected.total_shares).saturating_add(management_shares);
                let profit = fee_assets_base.saturating_sub(expected.fee_anchor.total_assets);
                let performance_fee_assets = config
                    .fees
                    .performance
                    .fee_wad
                    .apply_floored(Number::from(profit));
                let performance_shares = templar_vault_kernel::compute_fee_shares_from_assets(
                    performance_fee_assets,
                    Number::from(current_assets),
                    supply_after_management,
                );
                expected.total_shares = supply_after_management
                    .saturating_add(performance_shares)
                    .as_u128_saturating();
                expected.fee_anchor = FeeAccrualAnchor::new(
                    current_assets,
                    templar_vault_kernel::TimestampNs(now_ns),
                );
                expected
            };

            let view = SorobanVaultContract::proxy_view(
                env.clone(),
                owner,
                3_333_333_333_333_333_333,
                2_222_222_222_222_222_222,
            )
            .unwrap();
            let conversions = view.2;
            assert_eq!(
                conversions.6 as u128,
                convert_to_assets_ceil(&expected_state, &config, 2_222_222_222_222_222_222)
            );
            assert_eq!(
                conversions.7 as u128,
                convert_to_shares_ceil(&expected_state, &config, 3_333_333_333_333_333_333)
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
        let proxy = VaultProxy::new(&env);

        let owner = soroban_sdk::Address::generate(&env);
        let receiver = soroban_sdk::Address::generate(&env);
        let operator = soroban_sdk::Address::generate(&env);

        env.as_contract(&contract_id, || {
            proxy
                .initialize(
                    curator.clone(),
                    register_runtime_contracts(&env, &contract_id, &curator).0,
                    asset.clone(),
                    share.clone(),
                )
                .unwrap();

            let mut storage = SorobanStorage::new(&env);
            let state = VaultState {
                total_assets: 1_500,
                total_shares: 1_000,
                idle_assets: 1_500,
                ..Default::default()
            };
            storage.save_state(&state).expect("save state");
        });

        asset_admin_client.mint(&contract_id, &1_500);
        share_admin_client.mint(&owner, &1_000);

        let without_approval = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.as_contract(&contract_id, || {
                proxy.withdraw(&receiver, &owner, &operator, 500)
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
                proxy.withdraw(&receiver, &owner, &operator, 500)
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
                register_runtime_contracts(&env, &contract_id, &curator).0,
                asset.clone(),
                share.clone(),
                0,
                0,
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
                execute_command(
                    &env,
                    &VaultCommand::DepositWithMin {
                        owner: sdk_text(&owner),
                        receiver: sdk_text(&receiver),
                        assets: deposit_assets,
                        min_shares_out: 0,
                    },
                )
            })
            .expect("deposit_with_min should succeed");
        let VaultCommandResult::I128(minted) = minted else {
            panic!("expected i128 result")
        };
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
        assert_eq!(
            env.as_contract(&contract_id, || env
                .storage()
                .instance()
                .get(&VaultDataKey::VirtualOffsetsLocked)),
            Some(true)
        );
    }

    #[test]
    fn test_abort_withdrawing_command_recovers_public_stuck_withdrawal() {
        use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
        use soroban_sdk::token::StellarAssetClient;

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set(LedgerInfo {
            timestamp: 1,
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
        let share_client = soroban_sdk::token::Client::new(&env, &share);

        let owner = soroban_sdk::Address::generate(&env);
        let deposit_assets = (MIN_WITHDRAWAL_ASSETS.saturating_mul(2)) as i128;

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                curator.clone(),
                asset.clone(),
                share.clone(),
                0,
                0,
            )
            .unwrap();
        });

        asset_admin_client.mint(&owner, &deposit_assets);

        env.as_contract(&contract_id, || {
            assert_eq!(
                execute_command(
                    &env,
                    &VaultCommand::DepositWithMin {
                        owner: sdk_text(&owner),
                        receiver: sdk_text(&owner),
                        assets: deposit_assets,
                        min_shares_out: 0,
                    },
                )
                .unwrap(),
                VaultCommandResult::I128(deposit_assets)
            );
            assert_eq!(
                execute_command(
                    &env,
                    &VaultCommand::RequestWithdraw {
                        owner: sdk_text(&owner),
                        receiver: sdk_text(&owner),
                        shares: deposit_assets,
                        min_assets_out: 0,
                    },
                )
                .unwrap(),
                VaultCommandResult::U64(0)
            );
        });

        assert_eq!(share_client.balance(&owner), 0);
        assert_eq!(share_client.balance(&contract_id), deposit_assets);

        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let mut state = storage
                .load_state()
                .unwrap()
                .expect("initialized vault state");
            state.idle_assets = MIN_WITHDRAWAL_ASSETS.saturating_sub(1);
            state.total_assets = state.idle_assets.saturating_add(state.external_assets);
            storage.save_state(&state).unwrap();
        });

        env.ledger().set(LedgerInfo {
            timestamp: templar_vault_kernel::DEFAULT_COOLDOWN_NS / 1_000_000_000 + 3,
            protocol_version: 25,
            ..Default::default()
        });

        env.as_contract(&contract_id, || {
            assert_eq!(
                execute_command(
                    &env,
                    &VaultCommand::ExecuteWithdraw {
                        caller: sdk_text(&curator),
                    },
                )
                .expect_err("low-liquidity withdrawal should not start"),
                crate::error::ContractError::KernelError
            );
        });

        let op_id = env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let mut state = storage
                .load_state()
                .unwrap()
                .expect("initialized vault state");
            assert!(state.op_state.is_idle());
            let (request_id, owner, receiver, escrow_shares, expected_assets) = {
                let (request_id, head) = state.withdraw_queue.head().expect("withdrawal queued");
                (
                    request_id,
                    head.owner,
                    head.receiver,
                    head.escrow_shares,
                    head.expected_assets,
                )
            };
            let op_id = state.allocate_op_id();
            state.op_state = OpState::Withdrawing(WithdrawingState {
                op_id,
                request_id,
                index: 0,
                remaining: expected_assets,
                collected: 0,
                owner,
                receiver,
                escrow_shares,
            });
            storage.save_state(&state).unwrap();
            op_id
        });

        env.as_contract(&contract_id, || {
            assert_eq!(
                execute_command(
                    &env,
                    &VaultCommand::AbortWithdrawing {
                        caller: sdk_text(&curator),
                        op_id,
                    },
                )
                .unwrap(),
                VaultCommandResult::Unit
            );
        });

        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let state = storage
                .load_state()
                .unwrap()
                .expect("initialized vault state");
            assert!(state.op_state.is_idle());
            assert!(state.withdraw_queue.is_empty());
            assert_eq!(state.total_shares, deposit_assets as u128);
        });
        assert_eq!(share_client.balance(&owner), deposit_assets);
        assert_eq!(share_client.balance(&contract_id), 0);
    }

    #[test]
    fn test_execute_withdraw_command_returns_structured_status() {
        use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
        use soroban_sdk::token::StellarAssetClient;

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set(LedgerInfo {
            timestamp: 1,
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

        let owner = soroban_sdk::Address::generate(&env);
        let deposit_assets = (MIN_WITHDRAWAL_ASSETS.saturating_mul(2)) as i128;

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                curator.clone(),
                asset.clone(),
                share.clone(),
                0,
                0,
            )
            .unwrap();
        });

        asset_admin_client.mint(&owner, &deposit_assets);

        env.as_contract(&contract_id, || {
            execute_command(
                &env,
                &VaultCommand::DepositWithMin {
                    owner: sdk_text(&owner),
                    receiver: sdk_text(&owner),
                    assets: deposit_assets,
                    min_shares_out: 0,
                },
            )
            .unwrap();
            execute_command(
                &env,
                &VaultCommand::RequestWithdraw {
                    owner: sdk_text(&owner),
                    receiver: sdk_text(&owner),
                    shares: deposit_assets,
                    min_assets_out: 0,
                },
            )
            .unwrap();
        });

        env.ledger().set(LedgerInfo {
            timestamp: templar_vault_kernel::DEFAULT_COOLDOWN_NS / 1_000_000_000 + 3,
            protocol_version: 25,
            ..Default::default()
        });

        env.as_contract(&contract_id, || {
            assert_eq!(
                execute_command(
                    &env,
                    &VaultCommand::ExecuteWithdraw {
                        caller: sdk_text(&curator),
                    },
                )
                .unwrap(),
                VaultCommandResult::ExecuteWithdrawStatus(ExecuteWithdrawStatus {
                    op_state_before: OpState::Idle.kind_code(),
                    op_state_after: OpState::Idle.kind_code(),
                    assets_transferred: deposit_assets as u128,
                    events_emitted: 3,
                })
            );
        });
    }

    #[test]
    fn test_policy_state_getter() {
        let vault = create_test_vault();

        // Policy state should be initialized empty
        assert!(vault.policy_state().is_empty());
    }

    #[test]
    fn test_load_state_restores_policy_and_restrictions() {
        use soroban_sdk::testutils::Address as _;

        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(SorobanVaultContract, ());
        let curator = soroban_sdk::Address::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                governance,
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            let mut storage = SorobanStorage::new(&env);
            storage.save_state(&VaultState::default()).unwrap();
            storage.save_paused(false).unwrap();

            Storage::save_policy_state(&mut storage, &PolicyState::default()).unwrap();

            let restrictions =
                Restrictions::blacklist(alloc::vec![templar_vault_kernel::Address([9u8; 32])]);
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
    use soroban_sdk::testutils::Events;
    use soroban_sdk::{contract, contractimpl};
    use soroban_sdk::{Address, Env};
    use templar_vault_kernel::effects::KernelEffect;

    #[contract]
    struct EventTestContract;

    #[contractimpl]
    impl EventTestContract {
        pub fn noop(_env: Env) {}
    }

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
        EffectContext::new(
            1_000_000_000_000,
            templar_vault_kernel::Address([1u8; 32]),
            templar_vault_kernel::Address([2u8; 32]),
            templar_vault_kernel::Address([3u8; 32]),
        )
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
        assert_eq!(
            ctx.now_ns,
            templar_vault_kernel::TimestampNs(1_000_000_000_000)
        );
        assert_eq!(ctx.vault_address, templar_vault_kernel::Address([1u8; 32]));
        assert_eq!(ctx.asset_address, templar_vault_kernel::Address([2u8; 32]));
        assert_eq!(ctx.share_address, templar_vault_kernel::Address([3u8; 32]));
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
        let mut map = AddressMap::new();

        let kernel_addr = templar_vault_kernel::Address([1u8; 32]);
        let soroban_addr = Address::generate(&env);

        map.register(kernel_addr, soroban_addr.clone());

        let resolved = map.resolve(&kernel_addr);
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap(), &soroban_addr);

        // Unknown address
        let unknown = templar_vault_kernel::Address([2u8; 32]);
        assert!(map.resolve(&unknown).is_none());
    }

    #[test]
    fn test_emit_event_publishes_compact_payload_without_address_mapping() {
        use templar_vault_kernel::effects::KernelEvent;

        let env = test_env();
        let contract_id = env.register(EventTestContract, ());
        let share = TestSep41Token::new();
        let asset = TestSep41Token::new();
        let ctx = test_context();

        let effect = KernelEffect::EmitEvent {
            event: KernelEvent::DepositProcessed {
                owner: templar_vault_kernel::Address([1u8; 32]),
                receiver: templar_vault_kernel::Address([2u8; 32]),
                assets_in: 1,
                shares_out: 1,
            },
        };

        env.as_contract(&contract_id, || {
            let mut interpreter = SorobanEffectInterpreter::new(&env, &share, &asset);
            assert!(interpreter.execute_effect(&effect, &ctx).is_ok());
        });

        let events = env.events().all().filter_by_contract(&contract_id);
        assert_eq!(events.events().len(), 1);
    }

    #[test]
    fn kernel_event_payload_starts_with_codec_version_then_event_tag() {
        use crate::effects::{encode_kernel_event, KERNEL_EVENT_CODEC_VERSION};
        use templar_vault_kernel::effects::KernelEvent;

        let payload = encode_kernel_event(&KernelEvent::DepositProcessed {
            owner: templar_vault_kernel::Address([1u8; 32]),
            receiver: templar_vault_kernel::Address([2u8; 32]),
            assets_in: 3,
            shares_out: 4,
        });

        assert_eq!(payload[0], KERNEL_EVENT_CODEC_VERSION);
        assert_eq!(payload[1], 10);
    }
}

mod market_tests {
    use crate::error::RuntimeError;
    use crate::test_utils::{MarketRef, SettlementReceipt, SorobanCrossChainMarketAdapter};
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
    use crate::contract::helpers::{
        get_config_address, set_config_address, set_migration_in_progress,
    };
    use crate::contract::{adapter_for_market, supply_adapter_for_market, SorobanVaultContract};
    use crate::error::{ContractError, RuntimeError};
    use crate::storage::{
        SorobanStorage, SorobanStorageKey, Storage, SOROBAN_MAX_PENDING_WITHDRAWALS,
        SOROBAN_MAX_RESTRICTION_ADDRESSES,
    };
    use crate::test_utils::{fuzz_api, MemoryStorage};
    use alloc::string::{String as AllocString, ToString};
    use rstest::{fixture, rstest};
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address as SdkAddress, Bytes, Env, Symbol, Vec as SdkVec};
    use templar_curator_primitives::policy::cap_group::{CapGroup, CapGroupId, CapGroupRecord};
    use templar_curator_primitives::policy::state::{MarketConfig, OrderedMap};
    use templar_curator_primitives::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
    use templar_curator_primitives::PolicyState;
    use templar_soroban_governance::SorobanVaultGovernanceContract;
    use templar_soroban_shared_types::{
        GovernanceCommand, GOVERNANCE_CONFIG_KIND_ALLOCATORS,
        GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS, GOVERNANCE_CONFIG_KIND_CURATOR,
        GOVERNANCE_CONFIG_KIND_GOVERNANCE, GOVERNANCE_CONFIG_KIND_SENTINEL,
        GOVERNANCE_POLICY_KIND_CAP, GOVERNANCE_POLICY_KIND_GROUP, GOVERNANCE_POLICY_KIND_PAUSED,
        GOVERNANCE_POLICY_KIND_REMOVE_MARKET, GOVERNANCE_POLICY_KIND_RESTRICTIONS,
        GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
    };
    use templar_vault_kernel::{
        Address as KernelAddress, AllocationPlanEntry, FeeAccrualAnchor, OpState,
        PendingWithdrawal, Restrictions, TimestampNs, VaultState, WithdrawQueue, WithdrawingState,
    };

    fn sdk_text(address: &SdkAddress) -> AllocString {
        AllocString::from_utf8(address.to_string().to_bytes().to_alloc_vec())
            .expect("valid address")
    }

    fn supply_queue_from_ids(ids: &[u32]) -> SupplyQueue {
        SupplyQueue::try_from_entries(
            ids.iter()
                .map(|target_id| SupplyQueueEntry::new(*target_id, 100).unwrap())
                .collect(),
            None,
        )
        .unwrap()
    }

    fn policy_state_with_supply_queue(ids: &[u32]) -> PolicyState {
        let mut policy_state = PolicyState::default();
        for target_id in ids {
            policy_state
                .set_market_config(*target_id, MarketConfig::new(true, 100, None))
                .unwrap();
        }
        policy_state
            .replace_supply_queue(supply_queue_from_ids(ids))
            .unwrap();
        policy_state
    }

    fn initialize_governance_test_contract(env: &Env, governance: &SdkAddress) {
        let curator = SdkAddress::generate(env);
        let asset_token = SdkAddress::generate(env);
        let share_token = SdkAddress::generate(env);
        set_config_address(env, &crate::contract::VaultDataKey::Curator, &curator);
        set_config_address(env, &crate::contract::VaultDataKey::Governance, governance);
        set_config_address(
            env,
            &crate::contract::VaultDataKey::AssetToken,
            &asset_token,
        );
        set_config_address(
            env,
            &crate::contract::VaultDataKey::ShareToken,
            &share_token,
        );
        set_config_address(
            env,
            &crate::contract::VaultDataKey::SkimRecipient,
            governance,
        );
        let mut storage = SorobanStorage::new(env);
        storage.save_state(&VaultState::default()).unwrap();
        storage.save_paused(false).unwrap();
    }

    fn adapter_contract(env: &Env) -> SdkAddress {
        env.register(SorobanVaultContract, ())
    }

    fn account_address(env: &Env) -> SdkAddress {
        SdkAddress::from_str(
            env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
    }

    fn store_allowed_adapters(env: &Env, adapters: &[SdkAddress]) {
        let mut values = SdkVec::new(env);
        for adapter in adapters {
            values.push_back(adapter.clone());
        }
        env.storage()
            .instance()
            .set(&crate::contract::VaultDataKey::AllowedAdapters, &values);
    }

    fn store_test_adapter_bindings(env: &Env, pairs: &[(u32, SdkAddress)]) {
        let mut bindings = soroban_sdk::Map::new(env);
        for (target_id, adapter) in pairs {
            bindings.set(*target_id, adapter.clone());
        }
        env.storage()
            .instance()
            .set(&crate::contract::VaultDataKey::AdapterBindings, &bindings);
    }

    fn register_runtime_contracts(
        env: &Env,
        contract_id: &SdkAddress,
        admin: &SdkAddress,
    ) -> (SdkAddress, SdkAddress, SdkAddress) {
        let governance = env.register(
            SorobanVaultGovernanceContract,
            (admin, contract_id, &(0u64)),
        );
        let asset = env
            .register_stellar_asset_contract_v2(SdkAddress::generate(env))
            .address();
        let share = env
            .register_stellar_asset_contract_v2(contract_id.clone())
            .address();
        (governance, asset, share)
    }

    fn execute_governance_command(
        env: &Env,
        contract_id: &SdkAddress,
        caller: &SdkAddress,
        command: &GovernanceCommand,
    ) {
        use soroban_sdk::{IntoVal, Symbol};

        let payload = Bytes::from_slice(env, &command.encode());
        env.invoke_contract::<()>(
            contract_id,
            &Symbol::new(env, "execute_governance"),
            (caller, &payload).into_val(env),
        );
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
        let state = VaultState::default();

        storage.save_state(&state).unwrap();
        assert!(storage.is_initialized());

        let loaded = storage.load_state().unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap(), state);
    }

    #[test]
    fn test_memory_storage_with_state() {
        let state = VaultState::default();
        let storage = MemoryStorage::with_state(state.clone());

        assert!(storage.is_initialized());
        assert_eq!(storage.get_state(), Some(&state));
    }

    #[test]
    fn test_memory_storage_clear() {
        let state = VaultState::default();
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
        let kernel_addr = templar_vault_kernel::Address([9u8; 32]);
        let soroban_addr = SdkAddress::generate(&env);

        storage.save_address(&kernel_addr, &soroban_addr).unwrap();
        let loaded = storage.load_address(&kernel_addr).unwrap();
        assert_eq!(loaded, Some(soroban_addr));
    }

    #[test]
    fn test_storage_key_variants() {
        let key1 = SorobanStorageKey::StateBlob;
        let key2 = SorobanStorageKey::PausedState;
        let key3 = SorobanStorageKey::Restrictions;

        assert_ne!(key1, key2);
        assert_ne!(key2, key3);
    }

    #[test]
    fn test_soroban_storage_key_constants_are_distinct() {
        // All Symbol constants should be distinct from each other
        let keys: [Symbol; 8] = [
            SorobanStorageKey::StateBlob,
            SorobanStorageKey::PolicyLocks,
            SorobanStorageKey::PolicySupplyQueue,
            SorobanStorageKey::PolicyMarkets,
            SorobanStorageKey::PolicyPrincipals,
            SorobanStorageKey::PolicyCapGroups,
            SorobanStorageKey::Restrictions,
            SorobanStorageKey::PausedState,
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
            assert!(storage.load_state_blob().unwrap().is_none());

            // Save state
            let kernel = VaultState {
                total_assets: 10000,
                total_shares: 5000,
                idle_assets: 2000,
                external_assets: 8000,
                next_op_id: 1,
                ..Default::default()
            };
            let mut storage_mut = SorobanStorage::new(&env);
            Storage::save_state(&mut storage_mut, &kernel).unwrap();

            // Now storage should be initialized
            assert!(storage.is_initialized());

            // Load and verify
            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded.total_assets, 10000);
            assert_eq!(loaded.total_shares, 5000);
            assert_eq!(loaded.idle_assets, 2000);
            assert_eq!(loaded.external_assets, 8000);
            assert_eq!(loaded.next_op_id, 1);
        });
    }

    #[rstest]
    fn soroban_storage_extends_live_address_book_entries_from_state(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let kernel_addr = KernelAddress([9u8; 32]);
            let sdk_addr = SdkAddress::generate(&env);

            storage.save_address(&kernel_addr, &sdk_addr).unwrap();
            let state = VaultState {
                withdraw_queue: WithdrawQueue::with_state(
                    alloc::vec![(
                        0,
                        PendingWithdrawal::new(kernel_addr, kernel_addr, 1, 1, TimestampNs(0),),
                    )],
                    0,
                    1,
                ),
                ..Default::default()
            };
            storage.save_state(&state).unwrap();

            assert_eq!(
                Storage::load_address(&storage, &kernel_addr).unwrap(),
                Some(sdk_addr)
            );
            storage.extend_ttl(1, 100);
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

            let owner = templar_vault_kernel::Address([1u8; 32]);
            let receiver = templar_vault_kernel::Address([2u8; 32]);
            state.op_state = OpState::Withdrawing(WithdrawingState {
                op_id: 7,
                request_id: 7,
                index: 1,
                remaining: 500,
                collected: 200,
                receiver,
                owner,
                escrow_shares: 700,
            });

            let mut pending = BTreeMap::new();
            pending.insert(
                3,
                PendingWithdrawal::new(
                    owner,
                    receiver,
                    700,
                    800,
                    templar_vault_kernel::TimestampNs(123),
                ),
            );
            state.withdraw_queue = WithdrawQueue::with_state(pending, 3, 4);
            state.total_assets = 1000;
            state.total_shares = 900;
            state.idle_assets = 100;
            state.external_assets = 900;
            state.next_op_id = 8;

            storage.save_state(&state).unwrap();

            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded, state);
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
            let state = VaultState::default();
            storage.save_state(&state).unwrap();

            // Verify via trait
            assert!(Storage::is_initialized(&storage));
            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded, state);
        });
    }

    #[rstest]
    fn test_soroban_storage_load_state_rejects_corrupted_blob(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            storage
                .save_state_blob(&alloc::vec![1, 2, 3, 4, 5])
                .unwrap();

            let err = Storage::load_state(&storage).unwrap_err();
            assert_eq!(err, RuntimeError::StorageError);
        });
    }

    #[test]
    fn storage_codec_roundtrip_state_blob_manual() {
        let mut state = VaultState {
            total_assets: 5_000,
            total_shares: 4_000,
            idle_assets: 1_000,
            external_assets: 4_000,
            fee_anchor: FeeAccrualAnchor::new(4_500, TimestampNs(123_000)),
            op_state: OpState::Withdrawing(WithdrawingState {
                op_id: 7,
                request_id: 11,
                index: 1,
                remaining: 200,
                collected: 100,
                receiver: KernelAddress([2u8; 32]),
                owner: KernelAddress([1u8; 32]),
                escrow_shares: 300,
            }),
            next_op_id: 8,
            ..Default::default()
        };
        state.withdraw_queue = WithdrawQueue::with_state(
            alloc::vec![(
                3,
                PendingWithdrawal::new(
                    KernelAddress([1u8; 32]),
                    KernelAddress([2u8; 32]),
                    300,
                    350,
                    TimestampNs(456_000),
                ),
            ),],
            3,
            4,
        );

        let encoded = fuzz_api::encode_state_blob_bytes(&state);
        let decoded = fuzz_api::decode_state_blob_bytes(&encoded).expect("state roundtrip");
        assert_eq!(decoded, state);
    }

    fn state_with_pending_withdrawals(count: u32) -> VaultState {
        let pending = (0..u64::from(count))
            .map(|id| {
                (
                    id,
                    PendingWithdrawal::new(
                        KernelAddress([1u8; 32]),
                        KernelAddress([2u8; 32]),
                        1,
                        1,
                        TimestampNs(id),
                    ),
                )
            })
            .collect::<alloc::vec::Vec<_>>();
        let mut state = VaultState::default();
        state.withdraw_queue = WithdrawQueue::with_state(pending, 0, u64::from(count));
        state
    }

    #[test]
    fn storage_codec_state_blob_requires_versioned_envelope() {
        let state = state_with_pending_withdrawals(1);
        let encoded = fuzz_api::encode_state_blob_bytes(&state);
        assert_eq!(&encoded[..3], b"TVS");

        let decoded = fuzz_api::decode_state_blob_bytes(&encoded).expect("versioned state");
        assert_eq!(decoded, state);

        assert!(fuzz_api::decode_state_blob_bytes(&encoded[5..]).is_err());

        let mut unsupported_version = encoded;
        unsupported_version[4] = 2;
        assert!(fuzz_api::decode_state_blob_bytes(&unsupported_version).is_err());
    }

    #[test]
    fn migrate_validates_current_version_state_blob() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let governance = SdkAddress::generate(&env);
        let asset = SdkAddress::generate(&env);
        let share = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            let mut storage = SorobanStorage::new(&env);
            let state = state_with_pending_withdrawals(2);
            Storage::save_state(&mut storage, &state).unwrap();
            env.storage()
                .instance()
                .set(&soroban_sdk::symbol_short!("migrate"), &true);

            SorobanVaultContract::migrate(env.clone(), governance).unwrap();

            assert_eq!(Storage::load_state(&storage).unwrap(), Some(state));
            assert_eq!(
                env.storage()
                    .instance()
                    .get::<_, bool>(&soroban_sdk::symbol_short!("migrate")),
                None
            );
        });
    }

    #[test]
    fn storage_codec_rejects_malformed_withdraw_queue_ids() {
        let state = state_with_pending_withdrawals(2);
        let mut encoded = fuzz_api::encode_state_blob_bytes(&state);
        let second_id_offset = 5 + 89 + 8 + 8 + 4 + 112;
        encoded[second_id_offset..second_id_offset + 8].copy_from_slice(&0u64.to_le_bytes());

        assert!(fuzz_api::decode_state_blob_bytes(&encoded).is_err());
    }

    #[rstest]
    fn soroban_storage_enforces_safe_withdraw_queue_cap(contract_env: (Env, soroban_sdk::Address)) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let safe = state_with_pending_withdrawals(SOROBAN_MAX_PENDING_WITHDRAWALS);
            assert!(fuzz_api::encode_state_blob_bytes(&safe).len() > 64 * 1024);
            storage.save_state(&safe).expect("safe queue cap persists");
            assert_eq!(Storage::load_state(&storage).unwrap(), Some(safe));

            let too_large = state_with_pending_withdrawals(SOROBAN_MAX_PENDING_WITHDRAWALS + 1);
            assert_eq!(
                storage.save_state(&too_large).unwrap_err(),
                RuntimeError::StorageError
            );
        });
    }

    #[rstest]
    fn soroban_storage_rejects_missing_withdraw_queue_page(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let state = state_with_pending_withdrawals(129);
            storage.save_state(&state).expect("paged queue persists");

            env.storage()
                .persistent()
                .remove(&(soroban_sdk::symbol_short!("wqpage"), 1u64));

            assert_eq!(
                Storage::load_state(&storage).unwrap_err(),
                RuntimeError::StorageError
            );
        });
    }

    #[test]
    fn storage_codec_roundtrip_restrictions() {
        let restrictions = Restrictions::blacklist(alloc::vec![
            KernelAddress([9u8; 32]),
            KernelAddress([8u8; 32]),
        ]);
        let encoded = fuzz_api::encode_restrictions_bytes(&restrictions);
        let decoded =
            fuzz_api::decode_restrictions_bytes(&encoded).expect("restrictions roundtrip");
        assert_eq!(decoded, restrictions);

        let mut trailing = encoded;
        trailing.push(0xff);
        assert!(fuzz_api::decode_restrictions_bytes(&trailing).is_err());
    }

    #[rstest]
    fn soroban_storage_rejects_oversized_restrictions_before_save(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let addresses = (0..SOROBAN_MAX_RESTRICTION_ADDRESSES)
                .map(|i| {
                    let mut raw = [0u8; 32];
                    raw[..8].copy_from_slice(&(i as u64).to_le_bytes());
                    KernelAddress(raw)
                })
                .collect::<alloc::vec::Vec<_>>();
            let restrictions = Some(Restrictions::blacklist(addresses.clone()));
            Storage::save_restrictions(&mut storage, &restrictions).unwrap();
            assert_eq!(
                Storage::load_restrictions(&storage).unwrap(),
                Some(Restrictions::blacklist(addresses))
            );

            let oversized = (0..=SOROBAN_MAX_RESTRICTION_ADDRESSES)
                .map(|i| {
                    let mut raw = [0u8; 32];
                    raw[..8].copy_from_slice(&(i as u64).to_le_bytes());
                    KernelAddress(raw)
                })
                .collect::<alloc::vec::Vec<_>>();
            let restrictions = Some(Restrictions::blacklist(oversized));

            assert_eq!(
                Storage::save_restrictions(&mut storage, &restrictions).unwrap_err(),
                RuntimeError::StorageError
            );
        });
    }

    #[rstest]
    #[case::empty(alloc::vec![])]
    #[case::tag_only(alloc::vec![0])]
    #[case::invalid_tag(alloc::vec![2, 0, 0, 0, 0])]
    #[case::truncated_payload(alloc::vec![1, 2, 0, 0, 0, 1, 2, 3])]
    fn storage_codec_restrictions_bad_inputs_do_not_panic(#[case] bad: alloc::vec::Vec<u8>) {
        let _ = fuzz_api::decode_restrictions_bytes(&bad);
    }

    #[test]
    fn storage_codec_decode_state_blob_never_panics_on_small_inputs() {
        for len in 0..128usize {
            let bytes = alloc::vec![0xA5; len];
            let _ = fuzz_api::decode_state_blob_bytes(&bytes);
        }
    }

    fn versioned_storage_bytes(kind: u8, payload: &[u8]) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec::Vec::with_capacity(5 + payload.len());
        bytes.extend_from_slice(b"TVS");
        bytes.push(kind);
        bytes.push(1);
        bytes.extend_from_slice(payload);
        bytes
    }

    #[rstest]
    #[case::supply_queue_entries(u32::MAX.to_le_bytes().to_vec())]
    #[case::supply_queue_max_plus_entries({
        let mut payload = alloc::vec::Vec::new();
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&u32::MAX.to_le_bytes());
        payload
    })]
    fn storage_codec_rejects_malformed_supply_queue_lengths(#[case] payload: alloc::vec::Vec<u8>) {
        let encoded = versioned_storage_bytes(3, &payload);
        assert!(fuzz_api::decode_supply_queue_bytes(&encoded).is_err());
    }

    #[rstest]
    #[case::allocating_plan(1u8, 20usize, "allocating")]
    #[case::refreshing_plan(3u8, 4usize, "refreshing")]
    fn storage_codec_rejects_malformed_op_state_plan_lengths(
        #[case] tag: u8,
        #[case] item_size: usize,
        #[case] name: &str,
    ) {
        let mut payload = alloc::vec::Vec::new();
        for _ in 0..5 {
            payload.extend_from_slice(&0u128.to_le_bytes());
        }
        payload.extend_from_slice(&0u64.to_le_bytes());
        payload.push(tag);
        payload.extend_from_slice(&7u64.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        if tag == 1 {
            payload.extend_from_slice(&0u128.to_le_bytes());
        }
        payload.extend_from_slice(&u32::MAX.to_le_bytes());
        payload.extend_from_slice(&alloc::vec![0u8; item_size - 1]);

        let encoded = versioned_storage_bytes(1, &payload);
        assert!(
            fuzz_api::decode_state_blob_bytes(&encoded).is_err(),
            "{name} oversized plan length must fail before preallocating"
        );
    }

    #[test]
    fn storage_codec_roundtrip_supply_queue_and_truncated_bytes_fail_cleanly() {
        let queue =
            templar_curator_primitives::policy::supply_queue::SupplyQueue::try_from_entries(
                alloc::vec![
                    AllocationPlanEntry::new(0, 100),
                    AllocationPlanEntry::new(1, 200),
                ]
                .into_iter()
                .map(|entry: AllocationPlanEntry| {
                    templar_curator_primitives::policy::supply_queue::SupplyQueueEntry::new(
                        entry.target_id,
                        entry.amount,
                    )
                    .expect("valid queue entry")
                })
                .collect(),
                None,
            )
            .expect("queue build");

        let encoded = fuzz_api::encode_supply_queue_bytes(&queue);
        let decoded = fuzz_api::decode_supply_queue_bytes(&encoded).expect("queue roundtrip");
        assert_eq!(decoded, queue);

        let mut trailing = encoded.clone();
        trailing.push(0xff);
        assert!(fuzz_api::decode_supply_queue_bytes(&trailing).is_err());

        for len in 0..encoded.len() {
            let _ = fuzz_api::decode_supply_queue_bytes(&encoded[..len]);
        }
    }

    #[test]
    fn storage_codec_roundtrip_policy_locks_and_truncated_bytes_fail_cleanly() {
        use templar_curator_primitives::policy::market_lock::{
            FencingToken, LeaseOwner, MarketLease, MarketLeaseRegistry,
        };
        use templar_curator_primitives::policy::state::OrderedMap;

        let mut leases = OrderedMap::new();
        let _ = leases.insert(
            7,
            MarketLease::from_parts(
                7,
                LeaseOwner(11),
                Some(22),
                TimestampNs(100),
                TimestampNs(200),
                FencingToken(1),
            ),
        );
        let _ = leases.insert(
            8,
            MarketLease::from_parts(
                8,
                LeaseOwner(12),
                None,
                TimestampNs(300),
                TimestampNs(500),
                FencingToken(2),
            ),
        );
        let registry = MarketLeaseRegistry::from_parts(leases, 9);

        let encoded = fuzz_api::encode_policy_locks_bytes(&registry);
        let decoded =
            fuzz_api::decode_policy_locks_bytes(&encoded).expect("policy locks roundtrip");
        assert_eq!(decoded, registry);

        let mut trailing = encoded.clone();
        trailing.push(0xff);
        assert!(fuzz_api::decode_policy_locks_bytes(&trailing).is_err());

        for len in 0..encoded.len() {
            let _ = fuzz_api::decode_policy_locks_bytes(&encoded[..len]);
        }
    }

    #[rstest]
    fn soroban_storage_rejects_partial_policy_state(contract_env: (Env, soroban_sdk::Address)) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let mut markets = OrderedMap::new();
            let _ = markets.insert(1, MarketConfig::new(true, 100, None));
            storage
                .save_policy_markets(&fuzz_api::encode_markets_bytes(&markets))
                .unwrap();

            assert_eq!(
                Storage::load_policy_state(&storage).unwrap_err(),
                RuntimeError::StorageError
            );
        });
    }

    #[test]
    fn storage_codec_roundtrip_markets_and_invalid_tags_fail_cleanly() {
        use alloc::string::String;
        use templar_curator_primitives::policy::cap_group::CapGroupId;
        use templar_curator_primitives::policy::state::{MarketConfig, OrderedMap};

        let mut markets = OrderedMap::new();
        let _ = markets.insert(1, MarketConfig::new(true, 100, None));
        let _ = markets.insert(
            2,
            MarketConfig::new(
                false,
                200,
                Some(CapGroupId::try_from(String::from("grp-a")).expect("cap group id")),
            ),
        );

        let encoded = fuzz_api::encode_markets_bytes(&markets);
        let decoded = fuzz_api::decode_markets_bytes(&encoded).expect("markets roundtrip");
        assert_eq!(decoded, markets);

        let mut trailing = encoded.clone();
        trailing.push(0xff);
        assert!(fuzz_api::decode_markets_bytes(&trailing).is_err());

        let mut bad_enabled = encoded.clone();
        if bad_enabled.len() > 9 {
            bad_enabled[8] = 2;
            let _ = fuzz_api::decode_markets_bytes(&bad_enabled);
        }

        let mut bad_cap_group_tag = encoded.clone();
        if let Some(last) = bad_cap_group_tag.last_mut() {
            *last = 2;
            let _ = fuzz_api::decode_markets_bytes(&bad_cap_group_tag);
        }
    }

    #[test]
    fn storage_codec_roundtrip_cap_groups_and_trailing_bytes_fail_cleanly() {
        let mut cap = CapGroup::default();
        cap.set_absolute_cap(Some(1_000));
        cap.set_relative_cap(Some(templar_vault_kernel::Wad::one() / 2));

        let mut cap_groups = OrderedMap::new();
        let _ = cap_groups.insert(
            CapGroupId::try_from(AllocString::from("grp-a")).expect("cap group id"),
            CapGroupRecord {
                cap,
                principal: 500,
            },
        );

        let encoded = fuzz_api::encode_cap_groups_bytes(&cap_groups);
        let decoded = fuzz_api::decode_cap_groups_bytes(&encoded).expect("cap groups roundtrip");
        let decoded_record = decoded
            .get(&CapGroupId::try_from(AllocString::from("grp-a")).expect("cap group id"))
            .expect("decoded cap group");
        assert_eq!(decoded_record.principal, 500);
        assert_eq!(decoded_record.cap.absolute_cap(), Some(1_000));
        assert_eq!(
            decoded_record.cap.relative_cap(),
            Some(templar_vault_kernel::Wad::one() / 2)
        );

        let mut trailing = encoded;
        trailing.push(0xff);
        assert!(fuzz_api::decode_cap_groups_bytes(&trailing).is_err());
    }

    #[test]
    fn storage_codec_roundtrip_principals_and_truncated_bytes_fail_cleanly() {
        let mut principals = OrderedMap::new();
        let _ = principals.insert(1, 111);
        let _ = principals.insert(2, u128::MAX - 5);

        let encoded = fuzz_api::encode_principals_bytes(&principals);
        let decoded = fuzz_api::decode_principals_bytes(&encoded).expect("principals roundtrip");
        assert_eq!(decoded, principals);

        let mut trailing = encoded.clone();
        trailing.push(0xff);
        assert!(fuzz_api::decode_principals_bytes(&trailing).is_err());

        for len in 0..encoded.len() {
            let _ = fuzz_api::decode_principals_bytes(&encoded[..len]);
        }
    }

    fn storage_codec_roundtrip_state_blob_for_op_state(op_state: OpState) {
        let withdraw_queue = WithdrawQueue::with_state(
            alloc::vec![
                (
                    1,
                    PendingWithdrawal::new(
                        KernelAddress([1u8; 32]),
                        KernelAddress([2u8; 32]),
                        10,
                        15,
                        TimestampNs(50),
                    ),
                ),
                (
                    2,
                    PendingWithdrawal::new(
                        KernelAddress([3u8; 32]),
                        KernelAddress([4u8; 32]),
                        20,
                        25,
                        TimestampNs(60),
                    ),
                ),
            ],
            1,
            3,
        );

        let state = VaultState {
            total_assets: 1_000,
            total_shares: 2_000,
            idle_assets: 300,
            external_assets: 700,
            fee_anchor: FeeAccrualAnchor::new(900, TimestampNs(1_000)),
            op_state,
            withdraw_queue,
            next_op_id: 10,
        };

        let encoded = fuzz_api::encode_state_blob_bytes(&state);
        let decoded = fuzz_api::decode_state_blob_bytes(&encoded).expect("state matrix roundtrip");
        assert_eq!(decoded, state);
    }

    #[rstest]
    #[case::idle(OpState::Idle)]
    #[case::allocating(OpState::Allocating(templar_vault_kernel::AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 30,
        plan: alloc::vec![AllocationPlanEntry::new(9, 30)],
    }))]
    #[case::withdrawing(OpState::Withdrawing(WithdrawingState {
        op_id: 2,
        request_id: 3,
        index: 1,
        remaining: 40,
        collected: 10,
        receiver: KernelAddress([5u8; 32]),
        owner: KernelAddress([6u8; 32]),
        escrow_shares: 50,
    }))]
    #[case::refreshing(OpState::Refreshing(templar_vault_kernel::RefreshingState {
        op_id: 4,
        index: 1,
        plan: alloc::vec![3, 4, 5],
    }))]
    #[case::payout(OpState::Payout(templar_vault_kernel::PayoutState {
        op_id: 5,
        request_id: 6,
        receiver: KernelAddress([7u8; 32]),
        amount: 70,
        owner: KernelAddress([8u8; 32]),
        escrow_shares: 80,
        burn_shares: 60,
    }))]
    fn storage_codec_roundtrip_state_blob_op_state_matrix(#[case] op_state: OpState) {
        storage_codec_roundtrip_state_blob_for_op_state(op_state);
    }

    #[rstest]
    fn test_soroban_storage_load_state_rejects_trailing_bytes(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let mut bytes = fuzz_api::encode_state_blob_bytes(&VaultState::default());
            bytes.push(0xff);
            storage.save_state_blob(&bytes).unwrap();

            let err = Storage::load_state(&storage).unwrap_err();
            assert_eq!(err, RuntimeError::StorageError);
        });
    }

    #[rstest]
    fn test_governance_config_updates_allowed_adapters_without_supply_queue_constraint(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.mock_all_auths_allowing_non_root_auth();
        let governance = SdkAddress::generate(&env);
        let updated_adapter_one = adapter_contract(&env);
        let updated_adapter_two = adapter_contract(&env);
        let updated_adapters = SdkVec::from_array(&env, [updated_adapter_one, updated_adapter_two]);

        env.as_contract(&contract_id, || {
            set_config_address(
                &env,
                &crate::contract::VaultDataKey::Governance,
                &governance,
            );
            let mut storage = SorobanStorage::new(&env);
            let policy_state = policy_state_with_supply_queue(&[1]);
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            env.storage().instance().set(
                &crate::contract::VaultDataKey::AllowedAdapters,
                &SdkVec::from_array(
                    &env,
                    [SdkAddress::generate(&env), SdkAddress::generate(&env)],
                ),
            );

            let updated = updated_adapters
                .iter()
                .map(|address| sdk_text(&address))
                .collect();
            let payload = soroban_sdk::Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernanceConfig {
                    kind: GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
                    primary: None,
                    many: Some(updated),
                    value_a: None,
                    value_b: None,
                }
                .encode(),
            );
            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
                .unwrap();

            assert_eq!(
                env.storage()
                    .instance()
                    .get(&crate::contract::VaultDataKey::AllowedAdapters),
                Some(updated_adapters)
            );
        });
    }

    #[rstest]
    fn test_governance_config_rejects_non_contract_allowed_adapter(
        contract_env: (Env, soroban_sdk::Address),
    ) {
        let (env, contract_id) = contract_env;
        env.mock_all_auths_allowing_non_root_auth();
        let governance = SdkAddress::generate(&env);
        let non_contract_adapter = account_address(&env);

        env.as_contract(&contract_id, || {
            set_config_address(
                &env,
                &crate::contract::VaultDataKey::Governance,
                &governance,
            );

            let updated = alloc::vec![sdk_text(&non_contract_adapter)];
            let payload = soroban_sdk::Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernanceConfig {
                    kind: GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
                    primary: None,
                    many: Some(updated),
                    value_a: None,
                    value_b: None,
                }
                .encode(),
            );

            assert_eq!(
                SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload),
                Err(ContractError::InvalidInput)
            );
        });
    }

    #[test]
    fn test_governance_supply_queue_binds_new_market_from_proposal_adapters() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let governance = SdkAddress::generate(&env);
        let adapter_for_market_one = adapter_contract(&env);
        let adapter_for_market_two = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            initialize_governance_test_contract(&env, &governance);
            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = policy_state_with_supply_queue(&[1]);
            policy_state
                .set_market_config(2, MarketConfig::new(true, 100, None))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_allowed_adapters(
                &env,
                &[
                    adapter_for_market_one.clone(),
                    adapter_for_market_two.clone(),
                ],
            );
            store_test_adapter_bindings(&env, &[(1, adapter_for_market_one.clone())]);
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(alloc::vec![1, 2]),
                    mode: None,
                    accounts: Some(alloc::vec![
                        sdk_text(&adapter_for_market_one),
                        sdk_text(&adapter_for_market_two),
                    ]),
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
                .unwrap();
            assert_eq!(adapter_for_market(&env, 1).unwrap(), adapter_for_market_one);
            assert_eq!(adapter_for_market(&env, 2).unwrap(), adapter_for_market_two);
        });
    }

    #[test]
    fn test_governance_rejects_new_supply_queue_market_without_adapter() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let governance = SdkAddress::generate(&env);
        let adapter_for_market_one = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            initialize_governance_test_contract(&env, &governance);
            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = policy_state_with_supply_queue(&[1]);
            policy_state
                .set_market_config(2, MarketConfig::new(true, 100, None))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_test_adapter_bindings(&env, &[(1, adapter_for_market_one.clone())]);
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(alloc::vec![1, 2]),
                    mode: None,
                    accounts: None,
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            assert_eq!(
                SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload),
                Err(ContractError::InvalidInput)
            );
        });
    }

    #[test]
    fn test_governance_rejects_new_supply_queue_adapter_not_allowed() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let governance = SdkAddress::generate(&env);
        let adapter_for_market_one = adapter_contract(&env);
        let disallowed_adapter = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            initialize_governance_test_contract(&env, &governance);
            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = policy_state_with_supply_queue(&[1]);
            policy_state
                .set_market_config(2, MarketConfig::new(true, 100, None))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_allowed_adapters(&env, &[adapter_for_market_one.clone()]);
            store_test_adapter_bindings(&env, &[(1, adapter_for_market_one.clone())]);
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(alloc::vec![1, 2]),
                    mode: None,
                    accounts: Some(alloc::vec![
                        sdk_text(&adapter_for_market_one),
                        sdk_text(&disallowed_adapter),
                    ]),
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            assert_eq!(
                SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload),
                Err(ContractError::InvalidInput)
            );
            assert_eq!(adapter_for_market(&env, 1).unwrap(), adapter_for_market_one);
            assert_eq!(
                adapter_for_market(&env, 2),
                Err(ContractError::InvalidInput)
            );
        });
    }

    #[test]
    fn test_governance_rejects_new_supply_queue_adapter_non_contract() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let governance = SdkAddress::generate(&env);
        let adapter_for_market_one = adapter_contract(&env);
        let non_contract_adapter = account_address(&env);

        env.as_contract(&contract_id, || {
            initialize_governance_test_contract(&env, &governance);
            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = policy_state_with_supply_queue(&[1]);
            policy_state
                .set_market_config(2, MarketConfig::new(true, 100, None))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_allowed_adapters(
                &env,
                &[adapter_for_market_one.clone(), non_contract_adapter.clone()],
            );
            store_test_adapter_bindings(&env, &[(1, adapter_for_market_one.clone())]);
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(alloc::vec![1, 2]),
                    mode: None,
                    accounts: Some(alloc::vec![
                        sdk_text(&adapter_for_market_one),
                        sdk_text(&non_contract_adapter),
                    ]),
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            assert_eq!(
                SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload),
                Err(ContractError::InvalidInput)
            );
            assert_eq!(adapter_for_market(&env, 1).unwrap(), adapter_for_market_one);
            assert_eq!(
                adapter_for_market(&env, 2),
                Err(ContractError::InvalidInput)
            );
        });
    }

    #[test]
    fn test_governance_rejects_supply_queue_adapter_rebinding() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let governance = SdkAddress::generate(&env);
        let adapter_for_market_one = adapter_contract(&env);
        let replacement_adapter = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            initialize_governance_test_contract(&env, &governance);
            let mut storage = SorobanStorage::new(&env);
            let policy_state = policy_state_with_supply_queue(&[1]);
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_allowed_adapters(
                &env,
                &[adapter_for_market_one.clone(), replacement_adapter.clone()],
            );
            store_test_adapter_bindings(&env, &[(1, adapter_for_market_one.clone())]);
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(alloc::vec![1]),
                    mode: None,
                    accounts: Some(alloc::vec![sdk_text(&replacement_adapter)]),
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            assert_eq!(
                SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload),
                Err(ContractError::InvalidInput)
            );
            assert_eq!(adapter_for_market(&env, 1).unwrap(), adapter_for_market_one);
        });
    }

    #[test]
    fn test_governance_rejects_supply_queue_reuse_of_non_contract_binding() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let governance = SdkAddress::generate(&env);
        let non_contract_adapter = account_address(&env);

        env.as_contract(&contract_id, || {
            initialize_governance_test_contract(&env, &governance);
            let mut storage = SorobanStorage::new(&env);
            let policy_state = policy_state_with_supply_queue(&[1]);
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_test_adapter_bindings(&env, &[(1, non_contract_adapter)]);
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(alloc::vec![1]),
                    mode: None,
                    accounts: None,
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            assert_eq!(
                SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload),
                Err(ContractError::InvalidInput)
            );
        });
    }

    #[test]
    fn test_governance_rejects_supply_queue_reuse_of_disallowed_binding() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let governance = SdkAddress::generate(&env);
        let allowed_adapter = adapter_contract(&env);
        let stale_adapter = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            initialize_governance_test_contract(&env, &governance);
            let mut storage = SorobanStorage::new(&env);
            let policy_state = policy_state_with_supply_queue(&[1]);
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_allowed_adapters(&env, &[allowed_adapter]);
            store_test_adapter_bindings(&env, &[(1, stale_adapter.clone())]);
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(alloc::vec![1]),
                    mode: None,
                    accounts: None,
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            assert_eq!(
                SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload),
                Err(ContractError::InvalidInput)
            );
            assert_eq!(adapter_for_market(&env, 1).unwrap(), stale_adapter);
        });
    }

    #[test]
    fn test_supply_adapter_lookup_rejects_removed_adapter_binding() {
        let env = Env::default();
        let contract_id = env.register(SorobanVaultContract, ());
        let allowed_adapter = adapter_contract(&env);
        let removed_adapter = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            store_allowed_adapters(&env, &[allowed_adapter]);
            store_test_adapter_bindings(&env, &[(1, removed_adapter.clone())]);

            assert_eq!(adapter_for_market(&env, 1).unwrap(), removed_adapter);
            assert_eq!(
                supply_adapter_for_market(&env, 1),
                Err(ContractError::InvalidInput)
            );
        });
    }

    #[test]
    fn test_supply_adapter_lookup_allows_live_adapter_binding() {
        let env = Env::default();
        let contract_id = env.register(SorobanVaultContract, ());
        let adapter = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            store_allowed_adapters(&env, &[adapter.clone()]);
            store_test_adapter_bindings(&env, &[(1, adapter.clone())]);

            assert_eq!(supply_adapter_for_market(&env, 1).unwrap(), adapter);
        });
    }

    #[test]
    fn test_governance_supply_queue_reorder_does_not_require_adapters() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let governance = SdkAddress::generate(&env);
        let adapter_for_market_one = adapter_contract(&env);
        let adapter_for_market_two = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            initialize_governance_test_contract(&env, &governance);
            let mut storage = SorobanStorage::new(&env);
            let policy_state = policy_state_with_supply_queue(&[1, 2]);
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_allowed_adapters(
                &env,
                &[
                    adapter_for_market_one.clone(),
                    adapter_for_market_two.clone(),
                ],
            );
            store_test_adapter_bindings(
                &env,
                &[
                    (1, adapter_for_market_one.clone()),
                    (2, adapter_for_market_two.clone()),
                ],
            );
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(alloc::vec![2, 1]),
                    mode: None,
                    accounts: None,
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
                .unwrap();
            assert_eq!(adapter_for_market(&env, 1).unwrap(), adapter_for_market_one);
            assert_eq!(adapter_for_market(&env, 2).unwrap(), adapter_for_market_two);
        });
    }

    #[test]
    fn test_governance_supply_queue_removal_preserves_adapter_binding() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let governance = SdkAddress::generate(&env);
        let adapter_for_market_one = adapter_contract(&env);
        let adapter_for_market_two = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            initialize_governance_test_contract(&env, &governance);
            let mut storage = SorobanStorage::new(&env);
            let policy_state = policy_state_with_supply_queue(&[1, 2]);
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_allowed_adapters(
                &env,
                &[
                    adapter_for_market_one.clone(),
                    adapter_for_market_two.clone(),
                ],
            );
            store_test_adapter_bindings(
                &env,
                &[
                    (1, adapter_for_market_one.clone()),
                    (2, adapter_for_market_two.clone()),
                ],
            );
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(alloc::vec![2]),
                    mode: None,
                    accounts: None,
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
                .unwrap();
            let bindings: soroban_sdk::Map<u32, SdkAddress> = env
                .storage()
                .instance()
                .get(&crate::contract::VaultDataKey::AdapterBindings)
                .expect("adapter bindings stored");
            assert_eq!(bindings.get(1).unwrap(), adapter_for_market_one);
            assert_eq!(bindings.get(2).unwrap(), adapter_for_market_two);
        });
    }

    #[test]
    fn test_adapter_lookup_requires_keyed_binding() {
        let env = Env::default();
        let contract_id = env.register(SorobanVaultContract, ());
        let adapter_for_market_one = adapter_contract(&env);
        let adapter_for_market_two = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            storage.save_state(&VaultState::default()).unwrap();
            storage.save_paused(false).unwrap();
            let policy_state = policy_state_with_supply_queue(&[1, 2]);
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            let adapters = SdkVec::from_array(
                &env,
                [
                    adapter_for_market_one.clone(),
                    adapter_for_market_two.clone(),
                ],
            );
            env.storage()
                .instance()
                .set(&crate::contract::VaultDataKey::AllowedAdapters, &adapters);

            assert_eq!(
                adapter_for_market(&env, 1),
                Err(ContractError::InvalidInput)
            );
        });
    }

    #[test]
    fn test_adapter_binding_survives_supply_queue_reorder() {
        let env = Env::default();
        let contract_id = env.register(SorobanVaultContract, ());
        let adapter_for_market_one = adapter_contract(&env);
        let adapter_for_market_two = adapter_contract(&env);

        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            storage.save_state(&VaultState::default()).unwrap();
            storage.save_paused(false).unwrap();
            let mut policy_state = policy_state_with_supply_queue(&[1, 2]);
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            let adapters = SdkVec::from_array(
                &env,
                [
                    adapter_for_market_one.clone(),
                    adapter_for_market_two.clone(),
                ],
            );
            env.storage()
                .instance()
                .set(&crate::contract::VaultDataKey::AllowedAdapters, &adapters);
            store_test_adapter_bindings(
                &env,
                &[
                    (1, adapter_for_market_one.clone()),
                    (2, adapter_for_market_two.clone()),
                ],
            );
            let bindings: soroban_sdk::Map<u32, SdkAddress> = env
                .storage()
                .instance()
                .get(&crate::contract::VaultDataKey::AdapterBindings)
                .expect("adapter bindings stored");
            assert_eq!(bindings.get(1).unwrap(), adapter_for_market_one);
            assert_eq!(bindings.get(2).unwrap(), adapter_for_market_two);

            assert_eq!(adapter_for_market(&env, 1).unwrap(), adapter_for_market_one);
            assert_eq!(adapter_for_market(&env, 2).unwrap(), adapter_for_market_two);

            policy_state
                .replace_supply_queue(supply_queue_from_ids(&[2, 1]))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();

            assert_eq!(adapter_for_market(&env, 1).unwrap(), adapter_for_market_one);
            assert_eq!(adapter_for_market(&env, 2).unwrap(), adapter_for_market_two);
        });
    }

    #[test]
    fn test_adapter_binding_survives_multiple_supply_queue_permutations() {
        let env = Env::default();
        let contract_id = env.register(SorobanVaultContract, ());
        let adapters = [
            adapter_contract(&env),
            adapter_contract(&env),
            adapter_contract(&env),
            adapter_contract(&env),
        ];
        let expected = [
            (10, adapters[0].clone()),
            (20, adapters[1].clone()),
            (30, adapters[2].clone()),
            (40, adapters[3].clone()),
        ];
        let permutations = [
            [40, 30, 20, 10],
            [20, 10, 40, 30],
            [30, 40, 10, 20],
            [10, 40, 20, 30],
        ];

        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            storage.save_state(&VaultState::default()).unwrap();
            storage.save_paused(false).unwrap();
            let mut policy_state = policy_state_with_supply_queue(&[10, 20, 30, 40]);
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
            store_test_adapter_bindings(
                &env,
                &[
                    (10, adapters[0].clone()),
                    (20, adapters[1].clone()),
                    (30, adapters[2].clone()),
                    (40, adapters[3].clone()),
                ],
            );

            for (market, adapter) in expected.iter() {
                assert_eq!(adapter_for_market(&env, *market).unwrap(), *adapter);
            }

            for permutation in permutations {
                policy_state
                    .replace_supply_queue(supply_queue_from_ids(&permutation))
                    .unwrap();
                Storage::save_policy_state(&mut storage, &policy_state).unwrap();
                for (market, adapter) in expected.iter() {
                    assert_eq!(adapter_for_market(&env, *market).unwrap(), *adapter);
                }
            }
        });
    }

    #[test]
    fn test_governance_policy_group_membership_empty_string_clears_membership() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
            set_config_address(
                &env,
                &crate::contract::VaultDataKey::Governance,
                &governance,
            );

            let mut storage = SorobanStorage::new(&env);
            storage.save_state(&VaultState::default()).unwrap();
            storage.save_paused(false).unwrap();
            let mut policy_state = PolicyState::default();
            let cap_group_id = CapGroupId::try_from("group-c".to_string()).unwrap();
            policy_state.set_cap_group_absolute_cap(cap_group_id.clone(), Some(100));
            policy_state
                .set_market_config(7, MarketConfig::new(true, 100, Some(cap_group_id.clone())))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();

            let payload = soroban_sdk::Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_GROUP,
                    target_ids: None,
                    mode: Some(2),
                    accounts: None,
                    market_id: Some(7),
                    cap_group_id: Some("".to_string()),
                    value: Some(0),
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );
            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
                .unwrap();

            let reloaded = Storage::load_policy_state(&storage)
                .unwrap()
                .unwrap_or_default();
            assert_eq!(
                reloaded
                    .market_config(7)
                    .and_then(|config| config.cap_group_id.clone()),
                None
            );
        });
    }

    #[test]
    fn test_execute_governance_rejects_sentinel_for_governance_only_commands() {
        use soroban_sdk::{IntoVal, Symbol};

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let governance = SdkAddress::generate(&env);
        let sentinel = SdkAddress::generate(&env);
        let asset = SdkAddress::generate(&env);
        let share = SdkAddress::generate(&env);
        let replacement_governance = SdkAddress::generate(&env);
        let skim_token = env
            .register_stellar_asset_contract_v2(curator.clone())
            .address();

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
            set_config_address(&env, &crate::contract::VaultDataKey::Sentinel, &sentinel);
            set_config_address(
                &env,
                &crate::contract::VaultDataKey::SkimRecipient,
                &governance,
            );
        });

        let sentinel_config = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &sentinel,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernanceConfig {
                        kind: GOVERNANCE_CONFIG_KIND_GOVERNANCE,
                        primary: Some(sdk_text(&replacement_governance)),
                        many: None,
                        value_a: None,
                        value_b: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(
            sentinel_config,
            Err(Ok(crate::error::ContractError::Unauthorized))
        );
        env.as_contract(&contract_id, || {
            assert_eq!(
                get_config_address(&env, &crate::contract::VaultDataKey::Governance).unwrap(),
                governance
            );
        });

        let token_admin = soroban_sdk::token::StellarAssetClient::new(&env, &skim_token);
        token_admin.mint(&contract_id, &10);
        let sentinel_skim = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &sentinel,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::Skim {
                        token: sdk_text(&skim_token),
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(
            sentinel_skim,
            Err(Ok(crate::error::ContractError::Unauthorized))
        );
        let token_client = soroban_sdk::token::Client::new(&env, &skim_token);
        assert_eq!(token_client.balance(&contract_id), 10);

        execute_governance_command(
            &env,
            &contract_id,
            &governance,
            &GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_GOVERNANCE,
                primary: Some(sdk_text(&replacement_governance)),
                many: None,
                value_a: None,
                value_b: None,
            },
        );
        env.as_contract(&contract_id, || {
            assert_eq!(
                get_config_address(&env, &crate::contract::VaultDataKey::Governance).unwrap(),
                replacement_governance
            );
        });
    }

    #[test]
    fn test_execute_governance_cap_increase_applies_to_runtime_policy() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = PolicyState::default();
            policy_state
                .set_market_config(7, MarketConfig::new(true, 100, None))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();

            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_CAP,
                    target_ids: None,
                    mode: None,
                    accounts: None,
                    market_id: Some(7),
                    cap_group_id: None,
                    value: Some(200),
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );
            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
                .unwrap();

            let reloaded = Storage::load_policy_state(&storage)
                .unwrap()
                .unwrap_or_default();
            assert_eq!(
                reloaded.market_config(7).map(|config| config.cap),
                Some(200)
            );
        });
    }

    #[test]
    fn test_execute_governance_supply_queue_applies_without_curator_role() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = PolicyState::default();
            policy_state
                .set_market_config(7, MarketConfig::new(true, 100, None))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();

            let target_ids = alloc::vec![7u32];
            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                    target_ids: Some(target_ids),
                    mode: None,
                    accounts: None,
                    market_id: None,
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );
            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
                .unwrap();

            let reloaded = Storage::load_policy_state(&storage)
                .unwrap()
                .unwrap_or_default();
            let queue = reloaded.supply_queue();
            assert_eq!(queue.entries().len(), 1);
            assert_eq!(queue.entries()[0].target_id, 7);
        });
    }

    #[test]
    fn test_governance_policy_execution_does_not_grant_curator_role() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            let bootstrap = crate::contract::helpers::load_vault_bootstrap(&env).unwrap();
            assert!(!bootstrap.auth.config().has_role(
                &crate::contract::helpers::kernel_address_from_sdk(&env, &governance),
                templar_curator_primitives::rbac::Role::Curator,
            ));
        });
    }

    #[test]
    fn test_sentinel_cannot_execute_governance_cap_policy() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let sentinel = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(env.clone(), curator, governance, asset, share, 0, 0)
                .unwrap();
            set_config_address(&env, &crate::contract::VaultDataKey::Sentinel, &sentinel);

            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = PolicyState::default();
            policy_state
                .set_market_config(7, MarketConfig::new(true, 100, None))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();

            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_CAP,
                    target_ids: None,
                    mode: None,
                    accounts: None,
                    market_id: Some(7),
                    cap_group_id: None,
                    value: Some(200),
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );

            assert!(
                SorobanVaultContract::execute_governance(env.clone(), sentinel, payload).is_err()
            );
        });
    }

    #[test]
    fn test_execute_governance_cap_creates_new_runtime_market_after_timelock() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_CAP,
                    target_ids: None,
                    mode: None,
                    accounts: None,
                    market_id: Some(9),
                    cap_group_id: None,
                    value: Some(123),
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );
            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
                .unwrap();

            let storage = SorobanStorage::new(&env);
            let reloaded = Storage::load_policy_state(&storage)
                .unwrap()
                .unwrap_or_default();
            assert_eq!(
                reloaded.market_config(9).map(|config| config.cap),
                Some(123)
            );
        });
    }

    #[test]
    fn test_execute_governance_separates_sentinel_pause_from_governance_unpause() {
        use soroban_sdk::{IntoVal, Symbol};

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);
        let sentinel = SdkAddress::generate(&env);
        let attacker = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
        });

        execute_governance_command(
            &env,
            &contract_id,
            &governance,
            &GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_SENTINEL,
                primary: Some(sdk_text(&sentinel)),
                many: None,
                value_a: None,
                value_b: None,
            },
        );

        let governance_pause = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &governance,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernancePolicy {
                        kind: GOVERNANCE_POLICY_KIND_PAUSED,
                        target_ids: None,
                        mode: Some(1),
                        accounts: None,
                        market_id: None,
                        cap_group_id: None,
                        value: None,
                        value_b: None,
                        value_c: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(
            governance_pause,
            Err(Ok(crate::error::ContractError::Unauthorized))
        );

        let attacker_pause = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &attacker,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernancePolicy {
                        kind: GOVERNANCE_POLICY_KIND_PAUSED,
                        target_ids: None,
                        mode: Some(1),
                        accounts: None,
                        market_id: None,
                        cap_group_id: None,
                        value: None,
                        value_b: None,
                        value_c: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(
            attacker_pause,
            Err(Ok(crate::error::ContractError::Unauthorized))
        );

        execute_governance_command(
            &env,
            &contract_id,
            &sentinel,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_PAUSED,
                target_ids: None,
                mode: Some(1),
                accounts: None,
                market_id: None,
                cap_group_id: None,
                value: None,
                value_b: None,
                value_c: None,
            },
        );
        env.as_contract(&contract_id, || {
            assert!(SorobanStorage::new(&env).is_paused());
        });

        let sentinel_unpause = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &sentinel,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernancePolicy {
                        kind: GOVERNANCE_POLICY_KIND_PAUSED,
                        target_ids: None,
                        mode: Some(0),
                        accounts: None,
                        market_id: None,
                        cap_group_id: None,
                        value: None,
                        value_b: None,
                        value_c: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(
            sentinel_unpause,
            Err(Ok(crate::error::ContractError::Unauthorized))
        );

        execute_governance_command(
            &env,
            &contract_id,
            &governance,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_PAUSED,
                target_ids: None,
                mode: Some(0),
                accounts: None,
                market_id: None,
                cap_group_id: None,
                value: None,
                value_b: None,
                value_c: None,
            },
        );
        env.as_contract(&contract_id, || {
            assert!(!SorobanStorage::new(&env).is_paused());
        });

        let invalid_pause_mode = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &sentinel,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernancePolicy {
                        kind: GOVERNANCE_POLICY_KIND_PAUSED,
                        target_ids: None,
                        mode: Some(2),
                        accounts: None,
                        market_id: None,
                        cap_group_id: None,
                        value: None,
                        value_b: None,
                        value_c: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(
            invalid_pause_mode,
            Err(Ok(crate::error::ContractError::InvalidInput))
        );
    }

    #[test]
    fn test_execute_governance_separates_sentinel_tightening_from_governance_relaxation() {
        use soroban_sdk::{IntoVal, Symbol};

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);
        let sentinel = SdkAddress::generate(&env);
        let attacker = SdkAddress::generate(&env);
        let restricted = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
        });

        execute_governance_command(
            &env,
            &contract_id,
            &governance,
            &GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_SENTINEL,
                primary: Some(sdk_text(&sentinel)),
                many: None,
                value_a: None,
                value_b: None,
            },
        );

        let governance_tightening = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &governance,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernancePolicy {
                        kind: GOVERNANCE_POLICY_KIND_RESTRICTIONS,
                        target_ids: None,
                        mode: Some(1),
                        accounts: Some(alloc::vec![sdk_text(&restricted)]),
                        market_id: None,
                        cap_group_id: None,
                        value: None,
                        value_b: None,
                        value_c: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(
            governance_tightening,
            Err(Ok(crate::error::ContractError::Unauthorized))
        );

        let attacker_tightening = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &attacker,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernancePolicy {
                        kind: GOVERNANCE_POLICY_KIND_RESTRICTIONS,
                        target_ids: None,
                        mode: Some(1),
                        accounts: Some(alloc::vec![sdk_text(&restricted)]),
                        market_id: None,
                        cap_group_id: None,
                        value: None,
                        value_b: None,
                        value_c: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(
            attacker_tightening,
            Err(Ok(crate::error::ContractError::Unauthorized))
        );

        execute_governance_command(
            &env,
            &contract_id,
            &sentinel,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_RESTRICTIONS,
                target_ids: None,
                mode: Some(1),
                accounts: Some(alloc::vec![sdk_text(&restricted)]),
                market_id: None,
                cap_group_id: None,
                value: None,
                value_b: None,
                value_c: None,
            },
        );
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            assert!(matches!(
                Storage::load_restrictions(&storage).unwrap(),
                Some(Restrictions::Blacklist(_))
            ));
        });

        let sentinel_relaxation = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &sentinel,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernancePolicy {
                        kind: GOVERNANCE_POLICY_KIND_RESTRICTIONS,
                        target_ids: None,
                        mode: Some(0),
                        accounts: Some(alloc::vec![]),
                        market_id: None,
                        cap_group_id: None,
                        value: None,
                        value_b: None,
                        value_c: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(
            sentinel_relaxation,
            Err(Ok(crate::error::ContractError::Unauthorized))
        );

        execute_governance_command(
            &env,
            &contract_id,
            &governance,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_RESTRICTIONS,
                target_ids: None,
                mode: Some(0),
                accounts: Some(alloc::vec![]),
                market_id: None,
                cap_group_id: None,
                value: None,
                value_b: None,
                value_c: None,
            },
        );
        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            assert_eq!(Storage::load_restrictions(&storage).unwrap(), None);
        });
    }

    #[test]
    fn test_execute_governance_group_cap_increase_and_membership_apply() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);
        let cap_group_id = CapGroupId::try_from("group-c".to_string()).unwrap();

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = PolicyState::default();
            policy_state.set_cap_group_absolute_cap(cap_group_id.clone(), Some(100));
            policy_state
                .set_market_config(7, MarketConfig::new(true, 100, None))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
        });

        execute_governance_command(
            &env,
            &contract_id,
            &governance,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_GROUP,
                target_ids: None,
                mode: Some(0),
                accounts: None,
                market_id: None,
                cap_group_id: Some("group-c".to_string()),
                value: Some(200),
                value_b: None,
                value_c: None,
            },
        );

        execute_governance_command(
            &env,
            &contract_id,
            &governance,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_GROUP,
                target_ids: None,
                mode: Some(2),
                accounts: None,
                market_id: Some(7),
                cap_group_id: Some("group-c".to_string()),
                value: None,
                value_b: None,
                value_c: None,
            },
        );

        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let reloaded = Storage::load_policy_state(&storage)
                .unwrap()
                .unwrap_or_default();
            assert_eq!(
                reloaded
                    .cap_group(&cap_group_id)
                    .and_then(|record| record.cap.absolute_cap()),
                Some(200)
            );
            assert_eq!(
                reloaded
                    .market_config(7)
                    .and_then(|config| config.cap_group_id.clone()),
                Some(cap_group_id)
            );
        });
    }

    #[test]
    fn test_execute_governance_unauthorized_caller_rejected_before_body_decode() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);
        let attacker = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(env.clone(), curator, governance, asset, share, 0, 0)
                .unwrap();
        });

        let malformed_skim_payload = Bytes::from_slice(&env, &[2, 0xff, 0xff, 0xff, 0xff]);
        let result = env.as_contract(&contract_id, || {
            SorobanVaultContract::execute_governance(env.clone(), attacker, malformed_skim_payload)
        });
        assert_eq!(result, Err(crate::error::ContractError::Unauthorized));
    }

    #[test]
    fn test_execute_governance_group_membership_requires_market_id() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);
        let cap_group_id = CapGroupId::try_from("group-c".to_string()).unwrap();

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = PolicyState::default();
            policy_state.set_cap_group_absolute_cap(cap_group_id.clone(), Some(100));
            policy_state
                .set_market_config(0, MarketConfig::new(true, 100, None))
                .unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();
        });

        let payload = Bytes::from_slice(
            &env,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_GROUP,
                target_ids: None,
                mode: Some(2),
                accounts: None,
                market_id: None,
                cap_group_id: Some("group-c".to_string()),
                value: None,
                value_b: None,
                value_c: None,
            }
            .encode(),
        );
        let result = env.as_contract(&contract_id, || {
            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
        });
        assert_eq!(result, Err(crate::error::ContractError::InvalidInput));

        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let reloaded = Storage::load_policy_state(&storage)
                .unwrap()
                .unwrap_or_default();
            assert_eq!(
                reloaded
                    .market_config(0)
                    .and_then(|config| config.cap_group_id.clone()),
                None
            );
        });
    }

    #[test]
    fn test_execute_governance_remove_market_with_principal_after_timelock() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();

            let mut storage = SorobanStorage::new(&env);
            let mut policy_state = PolicyState::default();
            policy_state
                .set_market_config(7, MarketConfig::new(true, 0, None))
                .unwrap();
            policy_state.set_principal(7, 50).unwrap();
            Storage::save_policy_state(&mut storage, &policy_state).unwrap();

            let payload = Bytes::from_slice(
                &env,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_REMOVE_MARKET,
                    target_ids: None,
                    mode: None,
                    accounts: None,
                    market_id: Some(7),
                    cap_group_id: None,
                    value: None,
                    value_b: None,
                    value_c: None,
                }
                .encode(),
            );
            SorobanVaultContract::execute_governance(env.clone(), governance.clone(), payload)
                .unwrap();

            let reloaded = Storage::load_policy_state(&storage)
                .unwrap()
                .unwrap_or_default();
            assert!(reloaded.market_config(7).is_none());
            assert_eq!(reloaded.principal_entry(7), None);
        });
    }

    #[test]
    fn test_execute_governance_bridge_happy_path() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let governance = env.register(
            SorobanVaultGovernanceContract,
            (&curator, &contract_id, &(0u64)),
        );
        let asset = env
            .register_stellar_asset_contract_v2(SdkAddress::generate(&env))
            .address();
        let share = env
            .register_stellar_asset_contract_v2(contract_id.clone())
            .address();
        let new_curator = SdkAddress::generate(&env);
        let new_governance = env.register(
            SorobanVaultGovernanceContract,
            (&curator, &contract_id, &(0u64)),
        );
        let sentinel = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
        });

        execute_governance_command(
            &env,
            &contract_id,
            &governance,
            &GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_CURATOR,
                primary: Some(sdk_text(&new_curator)),
                many: None,
                value_a: None,
                value_b: None,
            },
        );
        env.as_contract(&contract_id, || {
            assert_eq!(
                env.storage()
                    .instance()
                    .get(&crate::contract::VaultDataKey::Curator),
                Some(new_curator.clone())
            );
        });

        execute_governance_command(
            &env,
            &contract_id,
            &governance,
            &GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_GOVERNANCE,
                primary: Some(sdk_text(&new_governance)),
                many: None,
                value_a: None,
                value_b: None,
            },
        );
        env.as_contract(&contract_id, || {
            assert_eq!(
                env.storage()
                    .instance()
                    .get(&crate::contract::VaultDataKey::Governance),
                Some(new_governance.clone())
            );
        });

        execute_governance_command(
            &env,
            &contract_id,
            &new_governance,
            &GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_SENTINEL,
                primary: Some(sdk_text(&sentinel)),
                many: None,
                value_a: None,
                value_b: None,
            },
        );
        env.as_contract(&contract_id, || {
            assert_eq!(
                env.storage()
                    .instance()
                    .get(&crate::contract::VaultDataKey::Sentinel),
                Some(sentinel.clone())
            );
        });
    }

    #[test]
    fn test_execute_governance_config_rejected_while_migration_in_progress() {
        use soroban_sdk::{IntoVal, Symbol};

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let governance = SdkAddress::generate(&env);
        let asset = SdkAddress::generate(&env);
        let share = SdkAddress::generate(&env);
        let new_curator = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator.clone(),
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
            set_migration_in_progress(&env, true);
        });

        let payload = Bytes::from_slice(
            &env,
            &GovernanceCommand::SetGovernanceConfig {
                kind: GOVERNANCE_CONFIG_KIND_CURATOR,
                primary: Some(sdk_text(&new_curator)),
                many: None,
                value_a: None,
                value_b: None,
            }
            .encode(),
        );

        let err = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (&governance, &payload).into_val(&env),
        );
        assert_eq!(
            err,
            Err(Ok(crate::error::ContractError::MigrationNotAllowed))
        );

        env.as_contract(&contract_id, || {
            assert_eq!(
                env.storage()
                    .instance()
                    .get(&crate::contract::VaultDataKey::Curator),
                Some(curator)
            );
        });
    }

    #[test]
    fn test_governance_config_rejects_duplicate_address_lists() {
        use soroban_sdk::{IntoVal, Symbol};

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
        });

        for kind in [
            GOVERNANCE_CONFIG_KIND_ALLOCATORS,
            GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
        ] {
            let duplicated = SdkAddress::generate(&env);
            let command = GovernanceCommand::SetGovernanceConfig {
                kind,
                primary: None,
                many: Some(alloc::vec![sdk_text(&duplicated), sdk_text(&duplicated)]),
                value_a: None,
                value_b: None,
            };
            let err = env.try_invoke_contract::<(), crate::error::ContractError>(
                &contract_id,
                &Symbol::new(&env, "execute_governance"),
                (&governance, &Bytes::from_slice(&env, &command.encode())).into_val(&env),
            );
            assert_eq!(err, Err(Ok(crate::error::ContractError::InvalidInput)));
        }
    }

    #[test]
    fn test_governance_config_empty_lists_keep_clear_semantics() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);
        let adapter = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
            env.storage().instance().set(
                &crate::contract::VaultDataKey::AllowedAdapters,
                &SdkVec::from_array(&env, [adapter]),
            );
        });

        for kind in [
            GOVERNANCE_CONFIG_KIND_ALLOCATORS,
            GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
        ] {
            execute_governance_command(
                &env,
                &contract_id,
                &governance,
                &GovernanceCommand::SetGovernanceConfig {
                    kind,
                    primary: None,
                    many: Some(alloc::vec![]),
                    value_a: None,
                    value_b: None,
                },
            );
        }

        env.as_contract(&contract_id, || {
            let allocators: Option<SdkVec<SdkAddress>> = env
                .storage()
                .instance()
                .get(&crate::contract::VaultDataKey::Allocators);
            assert_eq!(
                allocators.expect("allocators list should be stored").len(),
                0
            );

            let adapters: Option<SdkVec<SdkAddress>> = env
                .storage()
                .instance()
                .get(&crate::contract::VaultDataKey::AllowedAdapters);
            assert_eq!(adapters, None);
        });
    }

    #[test]
    fn test_execute_governance_skim_rejected_while_migration_in_progress() {
        use soroban_sdk::{IntoVal, Symbol};

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);
        let foreign_token = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
            set_migration_in_progress(&env, true);
        });

        let payload = Bytes::from_slice(
            &env,
            &GovernanceCommand::Skim {
                token: sdk_text(&foreign_token),
            }
            .encode(),
        );

        let err = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (&governance, &payload).into_val(&env),
        );
        assert_eq!(
            err,
            Err(Ok(crate::error::ContractError::MigrationNotAllowed))
        );
    }

    #[test]
    fn test_execute_governance_rejects_unauthorized_callers() {
        use soroban_sdk::{IntoVal, Symbol};

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);
        let attacker = SdkAddress::generate(&env);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
        });

        let err = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &attacker,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernanceConfig {
                        kind: GOVERNANCE_CONFIG_KIND_CURATOR,
                        primary: Some(sdk_text(&SdkAddress::generate(&env))),
                        many: None,
                        value_a: None,
                        value_b: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(err, Err(Ok(crate::error::ContractError::Unauthorized)));

        let err = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &attacker,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::SetGovernancePolicy {
                        kind: GOVERNANCE_POLICY_KIND_PAUSED,
                        target_ids: None,
                        mode: Some(1),
                        accounts: None,
                        market_id: None,
                        cap_group_id: None,
                        value: None,
                        value_b: None,
                        value_c: None,
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(err, Err(Ok(crate::error::ContractError::Unauthorized)));

        let err = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (
                &attacker,
                &Bytes::from_slice(
                    &env,
                    &GovernanceCommand::Skim {
                        token: sdk_text(&SdkAddress::generate(&env)),
                    }
                    .encode(),
                ),
            )
                .into_val(&env),
        );
        assert_eq!(err, Err(Ok(crate::error::ContractError::Unauthorized)));
    }

    #[test]
    fn test_execute_governance_rejects_malformed_payload() {
        use soroban_sdk::{IntoVal, Symbol};

        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register(SorobanVaultContract, ());
        let curator = SdkAddress::generate(&env);
        let (governance, asset, share) = register_runtime_contracts(&env, &contract_id, &curator);

        env.as_contract(&contract_id, || {
            SorobanVaultContract::initialize(
                env.clone(),
                curator,
                governance.clone(),
                asset,
                share,
                0,
                0,
            )
            .unwrap();
        });

        let err = env.try_invoke_contract::<(), crate::error::ContractError>(
            &contract_id,
            &Symbol::new(&env, "execute_governance"),
            (&governance, &Bytes::from_slice(&env, &[0xff])).into_val(&env),
        );
        assert_eq!(err, Err(Ok(crate::error::ContractError::InvalidInput)));
    }
}
