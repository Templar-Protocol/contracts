//! Integration tests for the Soroban curator vault.
//!
//! These tests verify full flows: deposit -> allocate -> refresh -> withdraw.

use rstest::{fixture, rstest};
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger, LedgerInfo},
    token::StellarAssetClient,
    Address as SdkAddress, Bytes, Env, IntoVal, Symbol,
};
use std::string::String as AllocString;
use templar_curator_primitives::policy::cap_group::CapGroupId;
use templar_curator_primitives::policy::state::MarketConfig;
use templar_curator_primitives::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
use templar_soroban_governance::{GovernanceError, SorobanVaultGovernanceContract};
use templar_soroban_runtime::{
    contract::{
        ContractConfig, CuratorVault, SorobanVaultContract, VaultDataKey,
        SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS,
    },
    rbac::{RbacAuth, RbacConfig, Role},
    storage::SorobanStorage,
    test_utils::{begin_allocating, finish_allocating, MemoryStorage},
    EffectContext,
    EffectInterpreter,
    Storage, // Import the trait
};
use templar_soroban_shared_types::{
    DepositReceipt, EmptyReceipt, ExecuteWithdrawReceipt, GovernanceCommand, I128Receipt,
    VaultCommand, GOVERNANCE_CONFIG_KIND_ALLOCATORS, GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
    GOVERNANCE_CONFIG_KIND_CURATOR, GOVERNANCE_CONFIG_KIND_SENTINEL,
    GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS, GOVERNANCE_POLICY_KIND_CAP,
    GOVERNANCE_POLICY_KIND_FEES, GOVERNANCE_POLICY_KIND_GROUP, GOVERNANCE_POLICY_KIND_PAUSED,
    GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
};
use templar_vault_kernel::{
    apply_action, compute_fee_shares_from_assets, compute_management_fee_shares,
    effects::KernelEffect, total_assets_for_fee_accrual, Address, AllocatingState,
    AllocationPlanEntry, FeeAccrualAnchor, FeeSlot, FeesSpec, KernelAction, Number, OpState,
    Restrictions, VaultConfig, VaultState, Wad, MAX_PENDING, MIN_WITHDRAWAL_ASSETS,
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

fn sdk_wire(address: &soroban_sdk::Address) -> AllocString {
    AllocString::from_utf8(address.to_string().to_bytes().to_alloc_vec()).expect("valid address")
}

type ProxyCoreView = (
    (
        soroban_sdk::Address,
        soroban_sdk::Address,
        soroban_sdk::Address,
        soroban_sdk::Address,
    ),
    (i128, i128, bool),
    (i128, i128, i128, i128),
    (i128, u64, i128, i128, i128),
);
type ProxyPolicyView = (
    soroban_sdk::Vec<u32>,
    soroban_sdk::Vec<(soroban_sdk::String, i128, i128)>,
);
type ProxyPreviewView = (i128, i128, i128, i128, i128, i128, i128, i128);
type ProxyViewResponse = (ProxyCoreView, ProxyPolicyView, ProxyPreviewView);

// Test Helpers

fn test_config() -> ContractConfig {
    ContractConfig::new(
        Address([1u8; 32]),       // curator
        Address([9u8; 32]),       // vault_address
        vec![Address([3u8; 32])], // allocators
        Address([4u8; 32]),       // asset_address
        Address([5u8; 32]),       // share_address
    )
}

fn curator_addr() -> Address {
    Address([1u8; 32])
}

fn sentinel_addr() -> Address {
    Address([11u8; 32])
}

fn allocator_addr() -> Address {
    Address([3u8; 32])
}

fn user_addr() -> Address {
    Address([10u8; 32])
}

struct SorobanContractFixture {
    env: Env,
    contract_id: soroban_sdk::Address,
    curator: soroban_sdk::Address,
    asset_token: soroban_sdk::Address,
    share_token: soroban_sdk::Address,
}

struct VaultProxy<'a> {
    env: &'a Env,
}

impl<'a> VaultProxy<'a> {
    const fn new(env: &'a Env) -> Self {
        Self { env }
    }

    #[allow(
        clippy::type_complexity,
        reason = "test proxy mirrors compact contract ABI"
    )]
    fn view(
        &self,
        owner: soroban_sdk::Address,
        assets: i128,
        shares: i128,
    ) -> Result<ProxyViewResponse, templar_soroban_runtime::ContractError> {
        SorobanVaultContract::proxy_view(self.env.clone(), owner, assets, shares)
    }

    fn snapshot(&self) -> Result<(i128, i128, i128), templar_soroban_runtime::ContractError> {
        let core = self.view(soroban_sdk::Address::generate(self.env), 0, 0)?.0;
        Ok((core.2 .0, core.2 .1, core.2 .2))
    }

    fn total_assets(&self) -> Result<i128, templar_soroban_runtime::ContractError> {
        Ok(self
            .view(soroban_sdk::Address::generate(self.env), 0, 0)?
            .0
             .2
             .3)
    }

    fn governance(&self) -> Result<soroban_sdk::Address, templar_soroban_runtime::ContractError> {
        Ok(self
            .view(soroban_sdk::Address::generate(self.env), 0, 0)?
            .0
             .0
             .1)
    }

    fn virtual_offsets(&self) -> Result<(i128, i128), templar_soroban_runtime::ContractError> {
        let core = self.view(soroban_sdk::Address::generate(self.env), 0, 0)?.0;
        Ok((core.1 .0, core.1 .1))
    }

    fn preview_deposit(
        &self,
        assets: i128,
    ) -> Result<i128, templar_soroban_runtime::ContractError> {
        Ok(self
            .view(soroban_sdk::Address::generate(self.env), assets, 0)?
            .2
             .0)
    }

    fn preview_redeem(&self, shares: i128) -> Result<i128, templar_soroban_runtime::ContractError> {
        Ok(self
            .view(soroban_sdk::Address::generate(self.env), 0, shares)?
            .2
             .1)
    }

    fn preview_withdraw(
        &self,
        assets: i128,
    ) -> Result<i128, templar_soroban_runtime::ContractError> {
        Ok(self
            .view(soroban_sdk::Address::generate(self.env), assets, 0)?
            .2
             .7)
    }

    fn max_deposit(&self) -> Result<i128, templar_soroban_runtime::ContractError> {
        Ok(self
            .view(soroban_sdk::Address::generate(self.env), 0, 0)?
            .2
             .2)
    }

    fn max_mint(&self) -> Result<i128, templar_soroban_runtime::ContractError> {
        Ok(self
            .view(soroban_sdk::Address::generate(self.env), 0, 0)?
            .2
             .3)
    }

    fn max_withdraw(
        &self,
        owner: soroban_sdk::Address,
    ) -> Result<i128, templar_soroban_runtime::ContractError> {
        Ok(self.view(owner, 0, 0)?.2 .4)
    }

    fn max_redeem(
        &self,
        owner: soroban_sdk::Address,
    ) -> Result<i128, templar_soroban_runtime::ContractError> {
        Ok(self.view(owner, 0, 0)?.2 .5)
    }

    fn execute(
        &self,
        command: &VaultCommand,
    ) -> Result<Bytes, templar_soroban_runtime::ContractError> {
        let payload = Bytes::from_slice(self.env, &command.encode());
        SorobanVaultContract::execute(self.env.clone(), payload)
    }

    fn execute_unit(
        &self,
        command: &VaultCommand,
    ) -> Result<(), templar_soroban_runtime::ContractError> {
        let bytes = self.execute(command)?;
        EmptyReceipt::decode(&bytes.to_alloc_vec())
            .map(|_| ())
            .map_err(|_| templar_soroban_runtime::ContractError::InvalidInput)
    }

    fn execute_withdraw(
        &self,
        caller: &soroban_sdk::Address,
    ) -> Result<ExecuteWithdrawReceipt, templar_soroban_runtime::ContractError> {
        let bytes = self.execute(&VaultCommand::ExecuteWithdraw {
            caller: sdk_wire(caller),
        })?;
        ExecuteWithdrawReceipt::decode(&bytes.to_alloc_vec())
            .map_err(|_| templar_soroban_runtime::ContractError::InvalidInput)
    }

    fn execute_governance_unit(
        &self,
        caller: &soroban_sdk::Address,
        command: &GovernanceCommand,
    ) -> Result<(), templar_soroban_runtime::ContractError> {
        let payload = Bytes::from_slice(self.env, &command.encode());
        SorobanVaultContract::execute_governance(self.env.clone(), caller.clone(), payload)?;
        Ok(())
    }
}

#[fixture]
fn soroban_contract_fixture() -> SorobanContractFixture {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let asset_admin = soroban_sdk::Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(asset_admin)
        .address();
    let share = env
        .register_stellar_asset_contract_v2(contract_id.clone())
        .address();
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&curator, &contract_id, &(0u64)),
    );

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(
            env.clone(),
            curator.clone(),
            governance,
            asset.clone(),
            share.clone(),
            0,
            0,
        )
        .unwrap();
    });

    SorobanContractFixture {
        env,
        contract_id,
        curator,
        asset_token: asset,
        share_token: share,
    }
}

#[test]
fn runtime_initialize_rejects_non_contract_governance() {
    let env = Env::default();
    env.mock_all_auths();
    let vault = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let governance = SdkAddress::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    );
    let asset = env
        .register_stellar_asset_contract_v2(soroban_sdk::Address::generate(&env))
        .address();
    let share = env
        .register_stellar_asset_contract_v2(vault.clone())
        .address();

    let result = env.as_contract(&vault, || {
        SorobanVaultContract::initialize(env.clone(), curator, governance, asset, share, 0, 0)
    });

    assert_eq!(
        result,
        Err(templar_soroban_runtime::ContractError::InvalidInput)
    );
}

#[rstest]
#[case(GOVERNANCE_CONFIG_KIND_CURATOR, true)]
#[case(GOVERNANCE_CONFIG_KIND_SENTINEL, true)]
#[case(GOVERNANCE_CONFIG_KIND_ALLOCATORS, false)]
fn runtime_governance_config_rejects_sac_role_addresses(
    #[case] kind: u32,
    #[case] primary_role: bool,
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let proxy = VaultProxy::new(&env);
    let contract_role = env
        .register_stellar_asset_contract_v2(contract_id.clone())
        .address();

    env.as_contract(&contract_id, || {
        let governance = proxy.governance().unwrap();
        let many = if primary_role {
            None
        } else {
            Some(std::vec![sdk_wire(&contract_role)])
        };
        let command = GovernanceCommand::SetGovernanceConfig {
            kind,
            primary: primary_role.then(|| sdk_wire(&contract_role)),
            many,
            value_a: None,
            value_b: None,
        };
        assert_eq!(
            proxy.execute_governance_unit(&governance, &command),
            Err(templar_soroban_runtime::ContractError::InvalidInput)
        );
    });
}

#[rstest]
#[case(
    GovernanceCommand::SetGovernancePolicy {
        kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
        target_ids: Some(vec![0]),
        mode: Some(0),
        accounts: None,
        market_id: None,
        cap_group_id: None,
        value: None,
        value_b: None,
        value_c: None,
    }
)]
#[case(
    GovernanceCommand::SetGovernancePolicy {
        kind: GOVERNANCE_POLICY_KIND_CAP,
        target_ids: None,
        mode: None,
        accounts: None,
        market_id: Some(0),
        cap_group_id: None,
        value: Some(1_000),
        value_b: Some(1),
        value_c: None,
    }
)]
#[case(
    GovernanceCommand::SetGovernancePolicy {
        kind: GOVERNANCE_POLICY_KIND_PAUSED,
        target_ids: None,
        mode: Some(1),
        accounts: None,
        market_id: Some(0),
        cap_group_id: None,
        value: None,
        value_b: None,
        value_c: None,
    }
)]
fn runtime_governance_policy_rejects_irrelevant_fields(
    #[case] command: GovernanceCommand,
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let proxy = VaultProxy::new(&env);

    env.as_contract(&contract_id, || {
        let governance = proxy.governance().unwrap();
        assert_eq!(
            proxy.execute_governance_unit(&governance, &command),
            Err(templar_soroban_runtime::ContractError::InvalidInput)
        );
    });
}

#[rstest]
fn soroban_contract_vault_snapshot_matches_fields(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let proxy = VaultProxy::new(&env);

    env.as_contract(&contract_id, || {
        let (total_shares, idle_assets, external_assets) = proxy.snapshot().unwrap();
        assert_eq!(total_shares, 0);
        assert_eq!(idle_assets, 0);
        assert_eq!(external_assets, 0);
    });
}

fn preview_kernel_config(paused: bool, virtual_shares: u128, virtual_assets: u128) -> VaultConfig {
    VaultConfig {
        fees: FeesSpec::zero(),
        min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
        withdrawal_cooldown_ns: SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS,
        max_pending_withdrawals: MAX_PENDING as u32,
        paused,
        virtual_shares,
        virtual_assets,
    }
}

fn fee_aware_preview_state(env: &Env, mut state: VaultState, config: &VaultConfig) -> VaultState {
    let now_ns = env.ledger().timestamp().saturating_mul(1_000_000_000);
    let anchor = state.fee_anchor;

    let same_timestamp_performance_due = now_ns == anchor.timestamp_ns.as_u64()
        && config.fees.max_total_assets_growth_rate.is_none()
        && !config.fees.performance.fee_wad.is_zero()
        && state.total_assets > anchor.total_assets;
    if state.total_shares == 0
        || !config.fees.has_active_slot_fees()
        || now_ns < anchor.timestamp_ns.as_u64()
        || (now_ns == anchor.timestamp_ns.as_u64() && !same_timestamp_performance_due)
    {
        return state;
    }

    let current_assets = state.total_assets;
    let fee_assets_base = total_assets_for_fee_accrual(
        current_assets,
        anchor.total_assets,
        anchor.timestamp_ns.as_u64(),
        now_ns,
        config.fees.max_total_assets_growth_rate,
    );
    let management_shares = compute_management_fee_shares(
        fee_assets_base,
        current_assets,
        state.total_shares,
        config.fees.management.fee_wad,
        anchor.timestamp_ns.as_u64(),
        now_ns,
    );
    let supply_after_management =
        Number::from(state.total_shares).saturating_add(management_shares);
    let profit = fee_assets_base.saturating_sub(anchor.total_assets);
    let performance_fee_assets = config
        .fees
        .performance
        .fee_wad
        .apply_floored(Number::from(profit));
    let performance_shares = compute_fee_shares_from_assets(
        performance_fee_assets,
        Number::from(current_assets),
        supply_after_management,
    );

    state.total_shares = supply_after_management
        .saturating_add(performance_shares)
        .as_u128_saturating();
    state.fee_anchor =
        FeeAccrualAnchor::new(current_assets, templar_vault_kernel::TimestampNs(now_ns));
    state
}

fn mint_shares_from_deposit(
    state: VaultState,
    assets_in: u128,
    virtual_shares: u128,
    virtual_assets: u128,
) -> u128 {
    let owner = Address([1u8; 32]);
    let receiver = Address([2u8; 32]);
    let self_id = Address([9u8; 32]);
    let config = preview_kernel_config(false, virtual_shares, virtual_assets);
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
            now_ns: templar_vault_kernel::TimestampNs(1),
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
    let asset_token = soroban_contract_fixture.asset_token;
    let assets_in = 500u128;
    let proxy = VaultProxy::new(&env);
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);

    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        let empty_state = VaultState::default();
        storage.save_state(&empty_state).unwrap();

        let preview = proxy.preview_deposit(assets_in as i128).unwrap();
        let minted = mint_shares_from_deposit(empty_state, assets_in, 0, 0);
        assert_eq!(preview as u128, minted);
    });

    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        let state = VaultState {
            total_assets: 10_000,
            total_shares: 8_000,
            idle_assets: 10_000,
            ..Default::default()
        };
        storage.save_state(&state).unwrap();
    });
    asset_admin_client.mint(&contract_id, &10_000);

    env.as_contract(&contract_id, || {
        let preview = proxy.preview_deposit(assets_in as i128).unwrap();
        let state = VaultState {
            total_assets: 10_000,
            total_shares: 8_000,
            idle_assets: 10_000,
            ..Default::default()
        };
        let minted = mint_shares_from_deposit(state, assets_in, 0, 0);
        assert_eq!(preview as u128, minted);
    });
}

#[rstest]
fn soroban_contract_preview_deposit_uses_configured_virtual_offsets(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let asset_token = soroban_contract_fixture.asset_token;
    let assets_in = 500u128;
    let virtual_shares = 123u128;
    let virtual_assets = 456u128;
    let proxy = VaultProxy::new(&env);
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);

    env.as_contract(&contract_id, || {
        let governance = proxy.governance().unwrap();
        proxy
            .execute_governance_unit(
                &governance,
                &GovernanceCommand::SetGovernanceConfig {
                    kind: GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS,
                    primary: None,
                    many: None,
                    value_a: Some(virtual_shares as i128),
                    value_b: Some(virtual_assets as i128),
                },
            )
            .unwrap();

        let mut storage = SorobanStorage::new(&env);
        let state = VaultState {
            total_assets: 10_000,
            total_shares: 8_000,
            idle_assets: 10_000,
            ..Default::default()
        };
        storage.save_state(&state).unwrap();

        let stored_offsets = proxy.virtual_offsets().unwrap();
        assert_eq!(
            stored_offsets,
            (virtual_shares as i128, virtual_assets as i128)
        );
    });
    asset_admin_client.mint(&contract_id, &10_000);

    env.as_contract(&contract_id, || {
        let preview = proxy.preview_deposit(assets_in as i128).unwrap();
        let state = VaultState {
            total_assets: 10_000,
            total_shares: 8_000,
            idle_assets: 10_000,
            ..Default::default()
        };
        let minted = mint_shares_from_deposit(state, assets_in, virtual_shares, virtual_assets);
        assert_eq!(preview as u128, minted);
    });
}

#[test]
fn governance_routes_upgrade_migrate_and_cancel_migration_to_real_vault() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let vault = env.register(SorobanVaultContract, ());
    let admin = soroban_sdk::Address::generate(&env);
    let asset_admin = soroban_sdk::Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(asset_admin)
        .address();
    let share = env
        .register_stellar_asset_contract_v2(vault.clone())
        .address();
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );
    let wasm_hash = env.deployer().upload_contract_wasm(Bytes::new(&env));

    env.as_contract(&vault, || {
        SorobanVaultContract::initialize(
            env.clone(),
            admin.clone(),
            governance.clone(),
            asset,
            share,
            0,
            0,
        )
        .unwrap();
    });

    let upgrade_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_upgrade(
            env.clone(),
            admin.clone(),
            wasm_hash.clone(),
        )
        .unwrap()
    });
    assert_eq!(
        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), upgrade_id)
        }),
        Err(GovernanceError::ProposalNotMature)
    );

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), upgrade_id).unwrap();
    });

    let migrate_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_migrate(env.clone(), admin.clone()).unwrap()
    });
    assert_eq!(
        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), migrate_id)
        }),
        Err(GovernanceError::ProposalNotMature)
    );

    env.ledger().set(LedgerInfo {
        timestamp: 112,
        protocol_version: 25,
        ..Default::default()
    });
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), migrate_id).unwrap();
    });

    env.ledger().set(LedgerInfo {
        timestamp: 200,
        protocol_version: 25,
        ..Default::default()
    });
    let second_upgrade_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_upgrade(
            env.clone(),
            admin.clone(),
            wasm_hash.clone(),
        )
        .unwrap()
    });
    env.ledger().set(LedgerInfo {
        timestamp: 206,
        protocol_version: 25,
        ..Default::default()
    });
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), second_upgrade_id)
            .unwrap();
    });

    let cancel_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_cancel_migration(env.clone(), admin.clone()).unwrap()
    });
    assert_eq!(
        env.as_contract(&governance, || {
            SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), cancel_id)
        }),
        Err(GovernanceError::ProposalNotMature)
    );

    env.ledger().set(LedgerInfo {
        timestamp: 212,
        protocol_version: 25,
        ..Default::default()
    });
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), cancel_id).unwrap();
    });

    env.as_contract(&vault, || {
        assert_eq!(
            SorobanVaultContract::migrate(env.clone(), governance.clone()),
            Err(templar_soroban_runtime::ContractError::InvalidState)
        );
    });
}

#[rstest]
fn soroban_contract_previews_simulate_configured_fee_accrual(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let asset_token = soroban_contract_fixture.asset_token;
    let proxy = VaultProxy::new(&env);
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);

    env.ledger().set(LedgerInfo {
        timestamp: 0,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&contract_id, || {
        let fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, Address([1u8; 32])),
            FeeSlot::new(Wad::one() / 5, Address([2u8; 32])),
            None,
        );
        let mut bytes = Vec::with_capacity(97);
        bytes.extend_from_slice(&fees.performance.fee_wad.as_u128_trunc().to_le_bytes());
        bytes.extend_from_slice(fees.performance.recipient.as_bytes());
        bytes.extend_from_slice(&fees.management.fee_wad.as_u128_trunc().to_le_bytes());
        bytes.extend_from_slice(fees.management.recipient.as_bytes());
        bytes.push(0);
        env.storage().instance().set(
            &templar_soroban_runtime::contract::VaultDataKey::FeesSpec,
            &Bytes::from_slice(&env, &bytes),
        );

        let mut storage = SorobanStorage::new(&env);
        let state = VaultState {
            total_assets: 1_500,
            total_shares: 1_000,
            idle_assets: 1_500,
            fee_anchor: FeeAccrualAnchor::new(1_000, templar_vault_kernel::TimestampNs(0)),
            ..Default::default()
        };
        storage.save_state(&state).unwrap();
    });
    asset_admin_client.mint(&contract_id, &1_500);

    env.as_contract(&contract_id, || {
        let fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, Address([1u8; 32])),
            FeeSlot::new(Wad::one() / 5, Address([2u8; 32])),
            None,
        );
        let config = VaultConfig {
            fees,
            min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
            withdrawal_cooldown_ns: SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS,
            max_pending_withdrawals: MAX_PENDING as u32,
            paused: false,
            virtual_shares: 0,
            virtual_assets: 0,
        };
        let state = VaultState {
            total_assets: 1_500,
            total_shares: 1_000,
            idle_assets: 1_500,
            fee_anchor: FeeAccrualAnchor::new(1_000, templar_vault_kernel::TimestampNs(0)),
            ..Default::default()
        };
        let expected_state = fee_aware_preview_state(&env, state, &config);
        let preview_deposit = proxy.preview_deposit(1_000).unwrap();
        let preview_withdraw = proxy.preview_withdraw(1_000).unwrap();
        let preview_redeem = proxy.preview_redeem(800).unwrap();

        assert_eq!(
            preview_deposit as u128,
            templar_vault_kernel::convert_to_shares(&expected_state, &config, 1_000)
        );
        assert_eq!(
            preview_withdraw as u128,
            templar_vault_kernel::convert_to_shares_ceil(&expected_state, &config, 1_000)
        );
        assert_eq!(
            preview_redeem as u128,
            templar_vault_kernel::convert_to_assets(&expected_state, &config, 800)
        );
    });
}

#[rstest]
fn soroban_contract_proxy_view_does_not_inflate_from_zero_fee_anchor(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let proxy = VaultProxy::new(&env);
    let owner = soroban_sdk::Address::generate(&env);

    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&contract_id, || {
        let fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 2, Address([1u8; 32])),
            FeeSlot::new(Wad::zero(), Address([2u8; 32])),
            None,
        );
        let mut bytes = Vec::with_capacity(97);
        bytes.extend_from_slice(&fees.performance.fee_wad.as_u128_trunc().to_le_bytes());
        bytes.extend_from_slice(fees.performance.recipient.as_bytes());
        bytes.extend_from_slice(&fees.management.fee_wad.as_u128_trunc().to_le_bytes());
        bytes.extend_from_slice(fees.management.recipient.as_bytes());
        bytes.push(0);
        env.storage().instance().set(
            &templar_soroban_runtime::contract::VaultDataKey::FeesSpec,
            &Bytes::from_slice(&env, &bytes),
        );

        let mut storage = SorobanStorage::new(&env);
        storage
            .save_state(&VaultState {
                total_assets: 1_000,
                total_shares: 1_000,
                idle_assets: 1_000,
                fee_anchor: FeeAccrualAnchor::new(0, templar_vault_kernel::TimestampNs(0)),
                ..Default::default()
            })
            .expect("save state");

        let total_shares = proxy.view(owner, 0, 0).unwrap().0 .2 .0;
        assert_eq!(total_shares, 1_000);
    });
}

#[rstest]
fn soroban_contract_proxy_view_rejects_overlarge_fee_anchor(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let proxy = VaultProxy::new(&env);
    let owner = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        storage
            .save_state(&VaultState {
                total_assets: 1_000,
                total_shares: 1_000,
                idle_assets: 0,
                external_assets: 1_000,
                fee_anchor: FeeAccrualAnchor::new(
                    i128::MAX as u128 + 1,
                    templar_vault_kernel::TimestampNs(123),
                ),
                ..Default::default()
            })
            .expect("save state");

        assert_eq!(
            proxy.view(owner, 0, 0),
            Err(templar_soroban_runtime::ContractError::ConversionOverflow)
        );
    });
}

#[rstest]
fn soroban_contract_proxy_view_reports_owner_idle_atomic_limits(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let asset_token = soroban_contract_fixture.asset_token;
    let share_token = soroban_contract_fixture.share_token;
    let proxy = VaultProxy::new(&env);
    let owner = soroban_sdk::Address::generate(&env);
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);
    let share_admin_client = StellarAssetClient::new(&env, &share_token);

    asset_admin_client.mint(&contract_id, &1_000);
    share_admin_client.mint(&owner, &600);

    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        storage
            .save_state(&VaultState {
                total_assets: 1_000,
                total_shares: 1_000,
                idle_assets: 1_000,
                ..Default::default()
            })
            .expect("save state");

        assert_eq!(proxy.max_withdraw(owner.clone()).unwrap(), 600);
        assert_eq!(proxy.max_redeem(owner).unwrap(), 600);
    });
}

#[rstest]
fn soroban_contract_proxy_view_reports_fee_growth_cap(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let proxy = VaultProxy::new(&env);
    let owner = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        let fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 5, Address([1u8; 32])),
            FeeSlot::new(Wad::one() / 10, Address([2u8; 32])),
            Some(Wad::one() / 20),
        );
        let mut bytes = Vec::with_capacity(113);
        bytes.extend_from_slice(&fees.performance.fee_wad.as_u128_trunc().to_le_bytes());
        bytes.extend_from_slice(fees.performance.recipient.as_bytes());
        bytes.extend_from_slice(&fees.management.fee_wad.as_u128_trunc().to_le_bytes());
        bytes.extend_from_slice(fees.management.recipient.as_bytes());
        bytes.push(1);
        bytes.extend_from_slice(
            &fees
                .max_total_assets_growth_rate
                .expect("growth cap configured")
                .as_u128_trunc()
                .to_le_bytes(),
        );
        env.storage().instance().set(
            &templar_soroban_runtime::contract::VaultDataKey::FeesSpec,
            &Bytes::from_slice(&env, &bytes),
        );

        let fee_info = proxy.view(owner, 0, 0).unwrap().0 .3;
        assert_eq!(fee_info.4, (Wad::one() / 20).as_u128_trunc() as i128);
    });
}

#[rstest]
fn soroban_contract_proxy_view_max_deposit_and_mint_respect_opposite_headroom(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let proxy = VaultProxy::new(&env);

    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        storage
            .save_state(&VaultState {
                total_assets: 2,
                total_shares: 1,
                idle_assets: 2,
                ..Default::default()
            })
            .expect("save state");

        assert_eq!(proxy.max_deposit().unwrap(), i128::MAX);
        assert_eq!(proxy.max_mint().unwrap(), i128::MAX);

        storage
            .save_state(&VaultState {
                total_assets: 1,
                total_shares: 2,
                idle_assets: 1,
                ..Default::default()
            })
            .expect("save state");

        let expected_max_deposit = (((i128::MAX as u128) * 2) / 3) as i128;
        assert_eq!(proxy.max_deposit().unwrap(), expected_max_deposit);
        assert_eq!(proxy.max_mint().unwrap(), i128::MAX);
    });
}

#[rstest]
fn soroban_contract_fee_aware_preview_fails_on_supply_overflow(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let proxy = VaultProxy::new(&env);

    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&contract_id, || {
        let fees = FeesSpec::new(
            FeeSlot::new(Wad::zero(), Address([1u8; 32])),
            FeeSlot::new(Wad::one(), Address([2u8; 32])),
            None,
        );
        let mut bytes = Vec::with_capacity(97);
        bytes.extend_from_slice(&fees.performance.fee_wad.as_u128_trunc().to_le_bytes());
        bytes.extend_from_slice(fees.performance.recipient.as_bytes());
        bytes.extend_from_slice(&fees.management.fee_wad.as_u128_trunc().to_le_bytes());
        bytes.extend_from_slice(fees.management.recipient.as_bytes());
        bytes.push(0);
        env.storage().instance().set(
            &templar_soroban_runtime::contract::VaultDataKey::FeesSpec,
            &Bytes::from_slice(&env, &bytes),
        );

        let mut storage = SorobanStorage::new(&env);
        storage
            .save_state(&VaultState {
                total_assets: u128::MAX,
                total_shares: u128::MAX,
                idle_assets: u128::MAX,
                fee_anchor: FeeAccrualAnchor::new(1, templar_vault_kernel::TimestampNs(1)),
                ..Default::default()
            })
            .expect("save state");

        assert_eq!(
            proxy.preview_deposit(1),
            Err(templar_soroban_runtime::ContractError::ConversionOverflow)
        );
    });
}

#[rstest]
fn soroban_contract_refresh_fees_command_updates_anchor() {
    let env = Env::default();
    env.mock_all_auths();
    let proxy = VaultProxy::new(&env);
    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&curator, &contract_id, &(0u64)),
    );
    let asset_admin = soroban_sdk::Address::generate(&env);
    let asset_sac = env.register_stellar_asset_contract_v2(asset_admin.clone());
    let asset_token = asset_sac.address();
    let share_sac = env.register_stellar_asset_contract_v2(contract_id.clone());
    let share_token = share_sac.address();
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);

    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(
            env.clone(),
            curator.clone(),
            governance.clone(),
            asset_token.clone(),
            share_token.clone(),
            0,
            0,
        )
        .unwrap();

        let mut storage = SorobanStorage::new(&env);
        storage
            .save_state(&VaultState {
                total_assets: 1_500,
                total_shares: 1_000,
                idle_assets: 1_500,
                fee_anchor: FeeAccrualAnchor::new(1_000, templar_vault_kernel::TimestampNs(0)),
                ..Default::default()
            })
            .expect("save state");
    });

    asset_admin_client.mint(&contract_id, &1500);

    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);
        proxy.execute_unit(&VaultCommand::RefreshFees).unwrap();

        let stored_state = storage
            .load_state()
            .expect("load state")
            .expect("state present");
        assert_eq!(stored_state.fee_anchor.total_assets, 1_500);
        assert_eq!(
            stored_state.fee_anchor.timestamp_ns,
            templar_vault_kernel::TimestampNs(100_000_000_000)
        );
    });
}

#[rstest]
fn soroban_contract_preview_withdraw_matches_kernel(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let asset_token = soroban_contract_fixture.asset_token;
    let proxy = VaultProxy::new(&env);
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);
    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        let state = VaultState {
            total_assets: 20_000,
            total_shares: 12_000,
            idle_assets: 20_000,
            ..Default::default()
        };
        storage.save_state(&state).unwrap();
    });
    asset_admin_client.mint(&contract_id, &20_000);

    env.as_contract(&contract_id, || {
        let assets_in: i128 = 1000;
        let shares_burned = proxy.preview_withdraw(assets_in).unwrap();
        assert_eq!(shares_burned, 601);

        let shares_in: i128 = 800;
        let assets_out = proxy.preview_redeem(shares_in).unwrap();
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
    let proxy = VaultProxy::new(&env);

    env.as_contract(&contract_id, || {
        let result = proxy.execute(&VaultCommand::ExecuteWithdraw {
            caller: sdk_wire(&user),
        });
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
    let proxy = VaultProxy::new(&env);

    env.as_contract(&contract_id, || {
        let state = VaultState {
            op_state: OpState::Allocating(AllocatingState {
                op_id: 1,
                index: 0,
                remaining: 0,
                plan: Vec::new(),
            }),
            ..Default::default()
        };
        let mut storage = SorobanStorage::new(&env);
        storage.save_state(&state).unwrap();
    });

    env.as_contract(&contract_id, || {
        let result = proxy.execute(&VaultCommand::ExecuteWithdraw {
            caller: sdk_wire(&user),
        });
        assert!(result.is_err());
    });
}

#[rstest]
fn soroban_contract_execute_withdraw_decodes_completed_receipt(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let curator = soroban_contract_fixture.curator;
    let asset_token = soroban_contract_fixture.asset_token;
    let owner = soroban_sdk::Address::generate(&env);
    let proxy = VaultProxy::new(&env);
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);
    let deposit_assets = (MIN_WITHDRAWAL_ASSETS.saturating_mul(2)) as i128;

    env.ledger().set(LedgerInfo {
        timestamp: 1,
        protocol_version: 25,
        ..Default::default()
    });
    asset_admin_client.mint(&owner, &deposit_assets);

    env.as_contract(&contract_id, || {
        proxy
            .execute(&VaultCommand::DepositWithMin {
                owner: sdk_wire(&owner),
                receiver: sdk_wire(&owner),
                assets: deposit_assets,
                min_shares_out: 0,
            })
            .unwrap();
        proxy
            .execute(&VaultCommand::RequestWithdraw {
                owner: sdk_wire(&owner),
                receiver: sdk_wire(&owner),
                shares: deposit_assets,
                min_assets_out: 0,
            })
            .unwrap();
    });

    env.ledger().set(LedgerInfo {
        timestamp: SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS / 1_000_000_000 + 3,
        protocol_version: 25,
        ..Default::default()
    });

    let receipt = env.as_contract(&contract_id, || {
        proxy
            .execute_withdraw(&curator)
            .expect("execute withdraw should return a typed receipt")
    });
    let ExecuteWithdrawReceipt::Completed {
        request_id,
        owner: receipt_owner,
        receiver,
        assets_out,
        shares_burned,
        ..
    } = receipt
    else {
        panic!("execute withdraw should complete the queued withdrawal");
    };

    assert_eq!(request_id, 0);
    assert_eq!(receipt_owner.as_str(), sdk_wire(&owner));
    assert_eq!(receiver.as_str(), sdk_wire(&owner));
    assert_eq!(assets_out, deposit_assets as u128);
    assert_eq!(shares_burned, deposit_assets as u128);

    env.as_contract(&contract_id, || {
        let state = SorobanStorage::new(&env)
            .load_state()
            .unwrap()
            .expect("state should remain persisted");
        assert!(state.withdraw_queue.is_empty());
        assert!(state
            .withdraw_queue
            .iter()
            .all(|(queued_request_id, _)| queued_request_id != request_id));
    });
}

#[rstest]
fn soroban_contract_execute_withdraw_decodes_no_payout_receipt(
    soroban_contract_fixture: SorobanContractFixture,
) {
    let env = soroban_contract_fixture.env;
    let contract_id = soroban_contract_fixture.contract_id;
    let curator = soroban_contract_fixture.curator;
    let asset_token = soroban_contract_fixture.asset_token;
    let owner = soroban_sdk::Address::generate(&env);
    let proxy = VaultProxy::new(&env);
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);
    let deposit_assets = (MIN_WITHDRAWAL_ASSETS.saturating_mul(2)) as i128;

    env.ledger().set(LedgerInfo {
        timestamp: 1,
        protocol_version: 25,
        ..Default::default()
    });
    asset_admin_client.mint(&owner, &deposit_assets);

    env.as_contract(&contract_id, || {
        proxy
            .execute(&VaultCommand::DepositWithMin {
                owner: sdk_wire(&owner),
                receiver: sdk_wire(&owner),
                assets: deposit_assets,
                min_shares_out: 0,
            })
            .unwrap();
        proxy
            .execute(&VaultCommand::RequestWithdraw {
                owner: sdk_wire(&owner),
                receiver: sdk_wire(&owner),
                shares: deposit_assets,
                min_assets_out: 0,
            })
            .unwrap();

        let mut storage = SorobanStorage::new(&env);
        let state = storage
            .load_state()
            .unwrap()
            .expect("withdraw request should persist state");
        let (_, pending) = state
            .withdraw_queue
            .head()
            .expect("withdraw request should be queued");
        Storage::save_restrictions(
            &mut storage,
            &Some(Restrictions::blacklist(vec![pending.owner])),
        )
        .unwrap();
    });

    env.ledger().set(LedgerInfo {
        timestamp: SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS / 1_000_000_000 + 3,
        protocol_version: 25,
        ..Default::default()
    });

    let receipt = env.as_contract(&contract_id, || {
        proxy
            .execute_withdraw(&curator)
            .expect("execute withdraw should return a typed receipt")
    });
    let ExecuteWithdrawReceipt::NoPayout { status } = receipt else {
        panic!("execute withdraw should skip the restricted request without payout");
    };

    assert_eq!(status.op_state_before, OpState::Idle.kind_code());
    assert_eq!(status.op_state_after, OpState::Idle.kind_code());
    assert_eq!(status.assets_transferred, 0);
    assert_eq!(status.events_emitted, 1);

    env.as_contract(&contract_id, || {
        let state = SorobanStorage::new(&env)
            .load_state()
            .unwrap()
            .expect("state should remain persisted");
        assert!(state.withdraw_queue.is_empty());
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
    for market_id in [0u32, 1, 2] {
        vault
            .policy_state_mut()
            .set_market_config(market_id, MarketConfig::new(true, i128::MAX as u128, None))
            .unwrap();
    }
    vault
        .policy_state_mut()
        .replace_supply_queue(
            SupplyQueue::try_from_entries(
                vec![
                    SupplyQueueEntry::new(0, 100).unwrap(),
                    SupplyQueueEntry::new(1, 100).unwrap(),
                    SupplyQueueEntry::new(2, 100).unwrap(),
                ],
                None,
            )
            .unwrap(),
        )
        .unwrap();
    vault
}

#[fixture]
fn vault() -> TestVault {
    create_test_vault()
}

type RbacVault = CuratorVault<MemoryStorage, RbacAuth, MockInterpreter>;

fn create_rbac_vault() -> RbacVault {
    let mut rbac_config = RbacConfig::with_curator(curator_addr());
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
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();
    vault
        .policy_state_mut()
        .replace_supply_queue(
            SupplyQueue::try_from_entries(vec![SupplyQueueEntry::new(0, 100).unwrap()], None)
                .unwrap(),
        )
        .unwrap();
    vault
}

#[fixture]
fn rbac_vault() -> RbacVault {
    create_rbac_vault()
}

struct RealSupplyFixture {
    env: Env,
    contract: SdkAddress,
    asset: SdkAddress,
    allocator: SdkAddress,
    /// Sole depositor; holds every share the fixture minted.
    user: SdkAddress,
    /// One adapter per market, index == market id. Markets never share an adapter.
    adapters: Vec<SdkAddress>,
    governance: SdkAddress,
}

fn invoke_vault(env: &Env, contract: &SdkAddress, command: &VaultCommand) -> Bytes {
    let payload = Bytes::from_slice(env, &command.encode());
    env.invoke_contract(
        contract,
        &Symbol::new(env, "execute"),
        (&payload,).into_val(env),
    )
}

fn try_invoke_vault(
    env: &Env,
    contract: &SdkAddress,
    command: &VaultCommand,
) -> Result<Bytes, templar_soroban_runtime::ContractError> {
    let payload = Bytes::from_slice(env, &command.encode());
    match env.try_invoke_contract::<Bytes, templar_soroban_runtime::ContractError>(
        contract,
        &Symbol::new(env, "execute"),
        (&payload,).into_val(env),
    ) {
        Ok(Ok(bytes)) => Ok(bytes),
        Err(Ok(error)) => Err(error),
        result => panic!("unexpected host invocation result: {result:?}"),
    }
}

fn invoke_governance(
    env: &Env,
    contract: &SdkAddress,
    caller: &SdkAddress,
    command: &GovernanceCommand,
) {
    env.as_contract(contract, || {
        VaultProxy::new(env)
            .execute_governance_unit(caller, command)
            .unwrap();
    });
}

#[contract]
struct LocalMarketAdapter;

#[contractimpl]
impl LocalMarketAdapter {
    pub fn supply(env: Env, _vault: SdkAddress, _asset: SdkAddress, amount: i128) {
        let key = Symbol::new(&env, "position");
        let position: i128 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(position + amount));
    }

    pub fn total_assets(env: Env, _asset: SdkAddress) -> i128 {
        env.storage().instance().set(
            &Symbol::new(&env, "reads"),
            &(Self::read_count(env.clone()) + 1),
        );
        let position: i128 = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "position"))
            .unwrap_or(0);
        let overage: i128 = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "overage"))
            .unwrap_or(0);
        position + overage
    }

    pub fn read_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "reads"))
            .unwrap_or(0)
    }

    pub fn progress_withdrawal(
        env: Env,
        vault: SdkAddress,
        asset: SdkAddress,
        amount: i128,
    ) -> i128 {
        soroban_sdk::token::Client::new(&env, &asset).transfer(
            &env.current_contract_address(),
            &vault,
            &amount,
        );
        let key = Symbol::new(&env, "position");
        let position: i128 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(position - amount));
        amount
    }
}

/// Builds a funded vault with `adapter_count` markets, each bound to its own adapter contract
/// and each genuinely holding `supply_each` through the real supply path. Markets never share
/// an adapter, so a reported position always belongs to exactly one market.
fn adapter_fixture(adapter_count: usize, market_cap: i128, supply_each: i128) -> RealSupplyFixture {
    let env = Env::default();
    env.mock_all_auths();
    let contract = env.register(SorobanVaultContract, ());
    let curator = SdkAddress::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(SdkAddress::generate(&env))
        .address();
    let share = env
        .register_stellar_asset_contract_v2(contract.clone())
        .address();
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&curator, &contract, &(0u64)),
    );
    let adapters: Vec<SdkAddress> = (0..adapter_count)
        .map(|_| env.register(LocalMarketAdapter, ()))
        .collect();
    let allocator = SdkAddress::generate(&env);
    let user = SdkAddress::generate(&env);
    env.ledger().set(LedgerInfo {
        timestamp: 1,
        protocol_version: 25,
        ..Default::default()
    });
    env.as_contract(&contract, || {
        SorobanVaultContract::initialize(
            env.clone(),
            curator.clone(),
            governance.clone(),
            asset.clone(),
            share,
            0,
            0,
        )
        .unwrap();
    });
    invoke_governance(
        &env,
        &contract,
        &governance,
        &GovernanceCommand::SetGovernanceConfig {
            kind: GOVERNANCE_CONFIG_KIND_ALLOCATORS,
            primary: None,
            many: Some(vec![sdk_wire(&allocator)]),
            value_a: None,
            value_b: None,
        },
    );
    invoke_governance(
        &env,
        &contract,
        &governance,
        &GovernanceCommand::SetGovernanceConfig {
            kind: GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
            primary: None,
            many: Some(adapters.iter().map(sdk_wire).collect::<Vec<AllocString>>()),
            value_a: None,
            value_b: None,
        },
    );
    for market in 0..adapter_count as u32 {
        invoke_governance(
            &env,
            &contract,
            &governance,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_CAP,
                target_ids: None,
                mode: None,
                accounts: None,
                market_id: Some(market),
                cap_group_id: None,
                value: Some(market_cap),
                value_b: None,
                value_c: None,
            },
        );
    }
    invoke_governance(
        &env,
        &contract,
        &governance,
        &GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
            target_ids: Some((0..adapter_count as u32).collect::<Vec<u32>>()),
            mode: None,
            accounts: Some(adapters.iter().map(sdk_wire).collect::<Vec<AllocString>>()),
            market_id: None,
            cap_group_id: None,
            value: None,
            value_b: None,
            value_c: None,
        },
    );
    StellarAssetClient::new(&env, &asset).mint(&user, &10_000);
    invoke_vault(
        &env,
        &contract,
        &VaultCommand::DepositWithMin {
            owner: sdk_wire(&user),
            receiver: sdk_wire(&user),
            assets: 10_000,
            min_shares_out: 0,
        },
    );
    for market in 0..adapter_count as u32 {
        let supplied = invoke_vault(
            &env,
            &contract,
            &VaultCommand::Allocate {
                caller: sdk_wire(&allocator),
                market,
                amount: supply_each,
                supply: true,
            },
        );
        assert_eq!(
            I128Receipt::decode(&supplied.to_alloc_vec()).unwrap().value,
            i128::from(market + 1) * supply_each,
            "external assets must accumulate one admitted step per market"
        );
    }
    RealSupplyFixture {
        env,
        contract,
        asset,
        allocator,
        user,
        adapters,
        governance,
    }
}

fn real_supply_fixture(supply_amount: i128) -> RealSupplyFixture {
    adapter_fixture(1, 2_000, supply_amount)
}

fn supply_at(
    fixture: &RealSupplyFixture,
    market: u32,
    amount: i128,
) -> Result<i128, templar_soroban_runtime::ContractError> {
    try_invoke_vault(
        &fixture.env,
        &fixture.contract,
        &VaultCommand::Allocate {
            caller: sdk_wire(&fixture.allocator),
            market,
            amount,
            supply: true,
        },
    )
    .map(|bytes| I128Receipt::decode(&bytes.to_alloc_vec()).unwrap().value)
}

fn supply(
    fixture: &RealSupplyFixture,
    amount: i128,
) -> Result<i128, templar_soroban_runtime::ContractError> {
    supply_at(fixture, 0, amount)
}

fn settle_at(
    fixture: &RealSupplyFixture,
    market: u32,
    amount: i128,
) -> Result<i128, templar_soroban_runtime::ContractError> {
    try_invoke_vault(
        &fixture.env,
        &fixture.contract,
        &VaultCommand::Allocate {
            caller: sdk_wire(&fixture.allocator),
            market,
            amount,
            supply: false,
        },
    )
    .map(|bytes| I128Receipt::decode(&bytes.to_alloc_vec()).unwrap().value)
}

fn settlement(
    fixture: &RealSupplyFixture,
    amount: i128,
) -> Result<i128, templar_soroban_runtime::ContractError> {
    settle_at(fixture, 0, amount)
}

fn exclude_supply(fixture: &RealSupplyFixture, exclusion: u32) {
    fixture.env.as_contract(&fixture.contract, || {
        let mut storage = SorobanStorage::new(&fixture.env);
        let mut policy = storage.load_policy_state().unwrap().unwrap();
        match exclusion {
            0 => policy.set_market_enabled(0, false).unwrap(),
            1 => policy
                .replace_supply_queue(SupplyQueue::try_from_entries(vec![], None).unwrap())
                .unwrap(),
            _ => policy.set_market_cap(0, 0).unwrap(),
        }
        storage.save_policy_state(&policy).unwrap();
    });
}

fn balances(fixture: &RealSupplyFixture) -> (i128, i128) {
    let token = soroban_sdk::token::Client::new(&fixture.env, &fixture.asset);
    (
        token.balance(&fixture.contract),
        token.balance(&fixture.adapters[0]),
    )
}

fn vault_invariants(fixture: &RealSupplyFixture) -> (bool, u128, u128, u128, Option<SdkAddress>) {
    fixture.env.as_contract(&fixture.contract, || {
        let storage = SorobanStorage::new(&fixture.env);
        let state = storage.load_state().unwrap().unwrap();
        let policy = storage.load_policy_state().unwrap().unwrap();
        let bindings = fixture
            .env
            .storage()
            .instance()
            .get::<_, soroban_sdk::Map<u32, SdkAddress>>(&VaultDataKey::AdapterBindings)
            .unwrap();
        (
            state.op_state.is_idle(),
            state.external_assets,
            state.idle_assets,
            policy.principal_for(0).unwrap(),
            bindings.get(0),
        )
    })
}

#[test]
fn overreported_supply_observation_is_refused_and_rolled_back() {
    // The adapter reports exposure outside the admitted allocation step. The observation guard
    // refuses before accounting; market and cap-group refusals are tested at admission.
    let fixture = real_supply_fixture(1_000);
    let balance_before = balances(&fixture);
    let invariant_before = vault_invariants(&fixture);
    assert_eq!(balance_before, (9_000, 1_000));
    assert_eq!(
        invariant_before,
        (true, 1_000, 9_000, 1_000, Some(fixture.adapters[0].clone()))
    );
    report_overage_at(&fixture, 0, 501);
    assert_eq!(
        supply(&fixture, 1_000),
        Err(templar_soroban_runtime::ContractError::InvalidState)
    );
    assert_eq!(balances(&fixture), balance_before);
    assert_eq!(vault_invariants(&fixture), invariant_before);
    report_overage_at(&fixture, 0, 0);
    assert_eq!(supply(&fixture, 1_000), Ok(2_000));
    assert_eq!(balances(&fixture), (8_000, 2_000));
    assert_eq!(
        vault_invariants(&fixture),
        (true, 2_000, 8_000, 2_000, Some(fixture.adapters[0].clone()))
    );
}

#[rstest]
#[case(0)]
#[case(1)]
#[case(2)]
fn supply_exclusion_preserves_binding_and_settlement(#[case] exclusion: u32) {
    let fixture = real_supply_fixture(1_000);
    let balance_before = balances(&fixture);
    let invariant_before = vault_invariants(&fixture);
    exclude_supply(&fixture, exclusion);
    assert_eq!(
        supply(&fixture, 1_000),
        Err(templar_soroban_runtime::ContractError::InvalidState)
    );
    assert_eq!(balances(&fixture), balance_before);
    assert_eq!(vault_invariants(&fixture), invariant_before);
    assert_eq!(settlement(&fixture, 1_000), Ok(0));
    assert_eq!(balances(&fixture), (10_000, 0));
    assert_eq!(
        vault_invariants(&fixture),
        (true, 0, 10_000, 0, Some(fixture.adapters[0].clone()))
    );
}

const REFRESH_CAP_GROUP: &str = "eng698-refresh";

fn set_refresh_group_cap(fixture: &RealSupplyFixture, absolute_cap: i128) {
    invoke_governance(
        &fixture.env,
        &fixture.contract,
        &fixture.governance,
        &GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_GROUP,
            target_ids: None,
            mode: Some(0),
            accounts: None,
            market_id: None,
            cap_group_id: Some(REFRESH_CAP_GROUP.to_string()),
            value: Some(absolute_cap),
            value_b: None,
            value_c: None,
        },
    );
}

fn set_refresh_group_membership(fixture: &RealSupplyFixture, market: u32) {
    invoke_governance(
        &fixture.env,
        &fixture.contract,
        &fixture.governance,
        &GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_GROUP,
            target_ids: None,
            mode: Some(2),
            accounts: None,
            market_id: Some(market),
            cap_group_id: Some(REFRESH_CAP_GROUP.to_string()),
            value: None,
            value_b: None,
            value_c: None,
        },
    );
}

fn set_refresh_group_relative_cap(fixture: &RealSupplyFixture, relative_cap: i128) {
    invoke_governance(
        &fixture.env,
        &fixture.contract,
        &fixture.governance,
        &GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_GROUP,
            target_ids: None,
            mode: Some(1),
            accounts: None,
            market_id: None,
            cap_group_id: Some(REFRESH_CAP_GROUP.to_string()),
            value: Some(relative_cap),
            value_b: None,
            value_c: None,
        },
    );
}

/// Reports `overage` from the adapter bound to `market`, so one market's NAV can move alone.
fn report_overage_at(fixture: &RealSupplyFixture, market: u32, overage: i128) {
    let adapter = &fixture.adapters[usize::try_from(market).unwrap()];
    fixture.env.as_contract(adapter, || {
        fixture
            .env
            .storage()
            .instance()
            .set(&Symbol::new(&fixture.env, "overage"), &overage);
    });
}

/// Booked group principal and its current effective cap, read straight from vault storage.
fn refresh_group_state(fixture: &RealSupplyFixture) -> (u128, u128) {
    let group_id = CapGroupId::try_from(REFRESH_CAP_GROUP.to_string()).unwrap();
    fixture.env.as_contract(&fixture.contract, || {
        let storage = SorobanStorage::new(&fixture.env);
        let state = storage.load_state().unwrap().unwrap();
        let policy = storage.load_policy_state().unwrap().unwrap();
        let record = policy.cap_groups().get(&group_id).unwrap();
        (
            record.principal,
            record.cap.effective_cap(state.total_assets),
        )
    })
}

/// Permissionless atomic exit by the fixture depositor; returns the assets paid from idle.
fn atomic_redeem(
    fixture: &RealSupplyFixture,
    receiver: &SdkAddress,
    shares: i128,
) -> Result<i128, templar_soroban_runtime::ContractError> {
    try_invoke_vault(
        &fixture.env,
        &fixture.contract,
        &VaultCommand::AtomicRedeem {
            owner: sdk_wire(&fixture.user),
            receiver: sdk_wire(receiver),
            operator: sdk_wire(&fixture.user),
            shares,
            min_assets_out: 0,
        },
    )
    .map(|bytes| I128Receipt::decode(&bytes.to_alloc_vec()).unwrap().value)
}

fn refresh(
    fixture: &RealSupplyFixture,
    markets: &[u32],
) -> Result<i128, templar_soroban_runtime::ContractError> {
    try_invoke_vault(
        &fixture.env,
        &fixture.contract,
        &VaultCommand::RefreshMarkets {
            caller: sdk_wire(&fixture.allocator),
            markets: markets.to_vec(),
        },
    )
    .map(|bytes| I128Receipt::decode(&bytes.to_alloc_vec()).unwrap().value)
}

/// Reads counted across every bound adapter, so one read per market is attributable.
fn adapter_reads(fixture: &RealSupplyFixture) -> u32 {
    fixture
        .adapters
        .iter()
        .map(|adapter| {
            fixture.env.invoke_contract::<u32>(
                adapter,
                &Symbol::new(&fixture.env, "read_count"),
                soroban_sdk::Vec::new(&fixture.env),
            )
        })
        .sum()
}

#[derive(Debug, PartialEq)]
struct RefreshSnapshot {
    idle: bool,
    next_op_id: u64,
    total_assets: u128,
    idle_assets: u128,
    external_assets: u128,
    principals: Vec<u128>,
    group_principal: u128,
    policy_blobs: [Option<Vec<u8>>; 4],
    adapter_reads: u32,
}

fn refresh_snapshot(fixture: &RealSupplyFixture) -> RefreshSnapshot {
    let mut snapshot = fixture.env.as_contract(&fixture.contract, || {
        let storage = SorobanStorage::new(&fixture.env);
        let state = storage.load_state().unwrap().unwrap();
        let policy = storage.load_policy_state().unwrap().unwrap();
        let group_id = CapGroupId::try_from(REFRESH_CAP_GROUP.to_string()).unwrap();
        let group_principal = policy
            .cap_groups()
            .get(&group_id)
            .map_or(0, |record| record.principal);
        RefreshSnapshot {
            idle: state.op_state.is_idle(),
            next_op_id: state.next_op_id,
            total_assets: state.total_assets,
            idle_assets: state.idle_assets,
            external_assets: state.external_assets,
            principals: vec![
                policy.principal_for(0).unwrap_or_default(),
                policy.principal_for(1).unwrap_or_default(),
                policy.principal_for(2).unwrap_or_default(),
            ],
            group_principal,
            policy_blobs: [
                storage.load_policy_markets().unwrap(),
                storage.load_policy_principals().unwrap(),
                storage.load_policy_cap_groups().unwrap(),
                storage.load_policy_supply_queue().unwrap(),
            ],
            adapter_reads: 0,
        }
    });
    snapshot.adapter_reads = adapter_reads(fixture);
    snapshot
}

fn set_market_cap(fixture: &RealSupplyFixture, market: u32, cap: i128) {
    invoke_governance(
        &fixture.env,
        &fixture.contract,
        &fixture.governance,
        &GovernanceCommand::SetGovernancePolicy {
            kind: GOVERNANCE_POLICY_KIND_CAP,
            target_ids: None,
            mode: None,
            accounts: None,
            market_id: Some(market),
            cap_group_id: None,
            value: Some(cap),
            value_b: None,
            value_c: None,
        },
    );
}

#[test]
fn refresh_per_market_cap_breach_rolls_back_and_boundary_persists() {
    // Two markets, two adapters, each reporting only its own position. The per-market ceiling is
    // the refusal left inside refresh, and it must roll back the adapter reads already done,
    // proving cross-contract atomicity through the public execute entry.
    let fixture = adapter_fixture(2, 2_000, 1_000);
    set_refresh_group_cap(&fixture, 10_000);
    set_refresh_group_membership(&fixture, 0);
    set_refresh_group_membership(&fixture, 1);
    set_market_cap(&fixture, 1, 500);

    let before = refresh_snapshot(&fixture);
    assert_eq!(
        before.principals,
        vec![1_000, 1_000, 0],
        "each adapter must report its own market position"
    );
    assert_eq!(before.group_principal, 2_000);

    assert_eq!(
        refresh(&fixture, &[0, 1]),
        Err(templar_soroban_runtime::ContractError::InvalidState)
    );
    assert_eq!(
        refresh_snapshot(&fixture),
        before,
        "the failing refresh must roll back vault, policy, and every adapter read"
    );

    set_market_cap(&fixture, 1, 2_000);
    assert_eq!(adapter_reads(&fixture), before.adapter_reads);

    assert_eq!(refresh(&fixture, &[0, 1]), Ok(2_000));
    let after = refresh_snapshot(&fixture);
    assert!(after.idle);
    assert_eq!(after.next_op_id, before.next_op_id + 1);
    assert_eq!(
        (after.total_assets, after.idle_assets, after.external_assets),
        (10_000, 8_000, 2_000)
    );
    assert_eq!(after.principals, vec![1_000, 1_000, 0]);
    assert_eq!(after.group_principal, 2_000);
    assert_eq!(
        after.adapter_reads,
        before.adapter_reads + 2,
        "each of the two markets must be read from its own adapter exactly once"
    );
}

#[test]
fn refresh_books_exposure_over_group_cap_and_group_cap_still_gates_supply() {
    // Both adapters already hold 1_000 each, so the absolute group cap (1_999) is below the
    // exposure that exists. Refresh reports reality and must book it; the group cap keeps gating
    // new supply and never blocks a withdrawal that reduces exposure.
    let fixture = adapter_fixture(2, 2_000, 1_000);
    set_refresh_group_cap(&fixture, 1_999);
    set_refresh_group_membership(&fixture, 0);
    set_refresh_group_membership(&fixture, 1);

    let before = refresh_snapshot(&fixture);
    assert_eq!(before.principals, vec![1_000, 1_000, 0]);
    assert_eq!(before.group_principal, 2_000);

    assert_eq!(
        refresh(&fixture, &[0, 1]),
        Ok(2_000),
        "observed exposure must be booked even above the group cap"
    );
    let after = refresh_snapshot(&fixture);
    assert!(after.idle);
    assert_eq!(after.principals, vec![1_000, 1_000, 0]);
    assert_eq!(after.group_principal, 2_000);
    assert_eq!(
        (after.total_assets, after.idle_assets, after.external_assets),
        (10_000, 8_000, 2_000)
    );
    assert_eq!(after.adapter_reads, before.adapter_reads + 2);

    let balances_before = balances(&fixture);
    assert_eq!(balances_before, (8_000, 1_000));
    assert_eq!(
        supply_at(&fixture, 0, 1),
        Err(templar_soroban_runtime::ContractError::InvalidState),
        "a breached group must refuse any extra supply"
    );
    assert_eq!(
        balances(&fixture),
        balances_before,
        "refusal must precede the transfer"
    );
    assert_eq!(
        refresh_snapshot(&fixture),
        after,
        "a refused supply must leave vault, policy, and adapter reads untouched"
    );

    assert_eq!(
        settle_at(&fixture, 0, 1_000),
        Ok(1_000),
        "an exposure-reducing withdrawal must stay permitted"
    );
    let reduced = refresh_snapshot(&fixture);
    assert_eq!(reduced.principals, vec![0, 1_000, 0]);
    assert_eq!(reduced.group_principal, 1_000);
    assert_eq!(
        (
            reduced.total_assets,
            reduced.idle_assets,
            reduced.external_assets
        ),
        (10_000, 9_000, 1_000)
    );
    assert_eq!(balances(&fixture), (9_000, 0));

    // The gate is live rather than stuck: headroom up to the cap is admitted, one unit more is not.
    assert_eq!(supply_at(&fixture, 0, 999), Ok(1_999));
    assert_eq!(balances(&fixture), (8_001, 999));
    assert_eq!(
        supply_at(&fixture, 0, 1),
        Err(templar_soroban_runtime::ContractError::InvalidState)
    );
    assert_eq!(refresh_snapshot(&fixture).group_principal, 1_999);
}

#[test]
fn refresh_books_outside_group_loss_then_atomic_redeem_pays_the_updated_nav() {
    // Three markets, three independently reporting adapters. Market 2 sits outside the capped
    // group and its own adapter reports a 1_000 NAV loss, which is what pushes the untouched group
    // above its relative cap. The refresh must book that loss, supply must stay gated, and the
    // permissionless atomic exit must price on the refreshed NAV instead of the stranded one.
    let fixture = adapter_fixture(3, 2_000, 1_000);
    set_refresh_group_relative_cap(&fixture, 200_000_000_000_000_000);
    set_refresh_group_membership(&fixture, 0);
    set_refresh_group_membership(&fixture, 1);
    assert_eq!(
        refresh_group_state(&fixture),
        (2_000, 2_000),
        "the group starts exactly at its 20% relative cap"
    );

    report_overage_at(&fixture, 2, -1_000);
    let before = refresh_snapshot(&fixture);
    assert_eq!(before.principals, vec![1_000, 1_000, 1_000]);
    assert_eq!(before.group_principal, 2_000);
    assert_eq!(
        (
            before.total_assets,
            before.idle_assets,
            before.external_assets
        ),
        (10_000, 7_000, 3_000)
    );

    assert_eq!(
        refresh(&fixture, &[0, 1, 2]),
        Ok(2_000),
        "an out-of-group loss must be booked even when it pushes the group over cap"
    );
    let after = refresh_snapshot(&fixture);
    assert!(after.idle);
    assert_eq!(after.principals, vec![1_000, 1_000, 0]);
    assert_eq!(
        (after.total_assets, after.idle_assets, after.external_assets),
        (9_000, 7_000, 2_000)
    );
    assert_eq!(after.group_principal, 2_000);
    assert_eq!(
        after.adapter_reads,
        before.adapter_reads + 3,
        "one honest read per market, from three different adapters"
    );
    assert_eq!(
        refresh_group_state(&fixture),
        (2_000, 1_800),
        "the loss alone is what leaves the group over its relative cap"
    );

    let balances_before = balances(&fixture);
    assert_eq!(balances_before, (7_000, 1_000));
    assert_eq!(
        supply_at(&fixture, 0, 1),
        Err(templar_soroban_runtime::ContractError::InvalidState),
        "recording a loss must not widen the supply gate"
    );
    assert_eq!(balances(&fixture), balances_before);
    assert_eq!(refresh_snapshot(&fixture), after);

    let receiver = SdkAddress::generate(&fixture.env);
    assert_eq!(
        atomic_redeem(&fixture, &receiver, 1_000),
        Ok(900),
        "1_000 shares must price at 0.9 assets on the refreshed NAV, not the stranded 1.0"
    );
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.asset).balance(&receiver),
        900
    );
    let exited = refresh_snapshot(&fixture);
    assert_eq!(
        (
            exited.total_assets,
            exited.idle_assets,
            exited.external_assets
        ),
        (8_100, 6_100, 2_000)
    );
    assert_eq!(exited.principals, vec![1_000, 1_000, 0]);
    assert_eq!(exited.group_principal, 2_000);
}

#[test]
fn refresh_after_permissionless_atomic_redemption_still_books_unchanged_exposure() {
    // No adapter moves: an idle atomic redemption only shrinks the denominator, which is enough
    // to put an unchanged group over its relative cap. A later refresh of the same principals
    // must still succeed, and the gate must still refuse new supply.
    let fixture = adapter_fixture(2, 2_000, 1_000);
    set_refresh_group_relative_cap(&fixture, 250_000_000_000_000_000);
    set_refresh_group_membership(&fixture, 0);
    set_refresh_group_membership(&fixture, 1);
    assert_eq!(refresh_group_state(&fixture), (2_000, 2_500));

    let receiver = SdkAddress::generate(&fixture.env);
    assert_eq!(atomic_redeem(&fixture, &receiver, 2_001), Ok(2_001));
    assert_eq!(
        StellarAssetClient::new(&fixture.env, &fixture.asset).balance(&receiver),
        2_001
    );
    let redeemed = refresh_snapshot(&fixture);
    assert_eq!(
        (
            redeemed.total_assets,
            redeemed.idle_assets,
            redeemed.external_assets
        ),
        (7_999, 5_999, 2_000)
    );
    assert_eq!(redeemed.group_principal, 2_000);
    assert_eq!(
        refresh_group_state(&fixture),
        (2_000, 1_999),
        "the redemption alone must be what pushes the group over cap"
    );

    assert_eq!(
        refresh(&fixture, &[0, 1]),
        Ok(2_000),
        "an unchanged observation must refresh while a permissionless exit tightened the cap"
    );
    let after = refresh_snapshot(&fixture);
    assert!(after.idle);
    assert_eq!(after.principals, vec![1_000, 1_000, 0]);
    assert_eq!(after.group_principal, 2_000);
    assert_eq!(
        (after.total_assets, after.idle_assets, after.external_assets),
        (7_999, 5_999, 2_000)
    );
    assert_eq!(after.adapter_reads, redeemed.adapter_reads + 2);

    assert_eq!(
        supply_at(&fixture, 0, 1),
        Err(templar_soroban_runtime::ContractError::InvalidState),
        "the tightened cap must still refuse new supply"
    );
    assert_eq!(refresh_snapshot(&fixture), after);
}

#[test]
fn refresh_idle_plus_external_overflow_is_invalid_state_and_rolls_back() {
    // Two independent adapters each report i128::MAX under matching market caps. Their sum still
    // fits u128, so only the idle + external aggregate overflows. The refusal must stay
    // InvalidState and leave vault, policy, and every adapter read untouched.
    let fixture = adapter_fixture(2, i128::MAX, 1_000);
    let huge_overage = i128::MAX - 1_000;
    report_overage_at(&fixture, 0, huge_overage);
    report_overage_at(&fixture, 1, huge_overage);

    let before = refresh_snapshot(&fixture);
    assert_eq!(before.idle_assets, 8_000);
    assert_eq!(before.external_assets, 2_000);

    assert_eq!(
        refresh(&fixture, &[0, 1]),
        Err(templar_soroban_runtime::ContractError::InvalidState),
        "idle + reported external overflow must keep the InvalidState public error"
    );
    assert_eq!(
        refresh_snapshot(&fixture),
        before,
        "the refused refresh must roll back vault, policy, and adapter reads"
    );

    report_overage_at(&fixture, 0, 0);
    report_overage_at(&fixture, 1, 0);
    assert_eq!(refresh(&fixture, &[0, 1]), Ok(2_000));
    let after = refresh_snapshot(&fixture);
    assert_eq!(
        (after.total_assets, after.idle_assets, after.external_assets),
        (10_000, 8_000, 2_000)
    );
    assert_eq!(after.principals, vec![1_000, 1_000, 0]);
}

// Deposit Flow Tests

#[rstest]
fn test_deposit_flow_single(mut vault: TestVault) {
    let user = user_addr();
    let receiver = Address([11u8; 32]);

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
    let receiver = Address([11u8; 32]);

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
    let receiver = Address([11u8; 32]);

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
    let receiver = Address([11u8; 32]);

    let result = vault.deposit(user, receiver, 0, 0, 100);
    assert!(result.is_err());
}

// Allocation Flow Tests

#[rstest]
fn test_allocation_flow_basic(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let allocator = allocator_addr();
    let user = user_addr();

    vault
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();
    vault
        .policy_state_mut()
        .set_market_config(1, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();

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
    let _op_id = begin_allocating(&mut vault, allocator, vec![(0, alloc_total)], 1000).unwrap();

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

    let op_id = begin_allocating(&mut vault, allocator, vec![(0, 5000)], 1000).unwrap();

    // Try to finish with wrong op_id
    let result = finish_allocating(&mut vault, allocator, op_id + 999);
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
    let result = begin_allocating(&mut rbac_vault, user, vec![(0, 5000)], 1000);
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
    let result = begin_allocating(&mut rbac_vault, allocator, vec![(0, 5000)], 1000);
    assert!(result.is_ok());
}

#[rstest]
fn test_rbac_curator_can_do_everything(mut rbac_vault: RbacVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let curator = curator_addr();

    rbac_vault
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();

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

    let sentinel = sentinel_addr();
    let user = user_addr();

    rbac_vault
        .set_restrictions(sentinel, Some(Restrictions::blacklist(vec![user])))
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
    assert_eq!(stored.total_assets, 1000);
    assert_eq!(stored.total_shares, 1000);
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
    assert_eq!(stored.external_assets, 5000);
    assert!(stored.op_state.is_idle());
}

// Effect Execution Tests

#[rstest]
fn test_deposit_emits_mint_effect(mut vault: TestVault) {
    let user = user_addr();
    let receiver = Address([11u8; 32]);

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

    vault
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();
    vault
        .policy_state_mut()
        .set_market_config(1, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();

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
    begin_allocating(&mut vault, allocator, vec![(0, 5000)], 1000).unwrap();

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
    let result = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1);
    assert!(result.is_ok());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
}

#[rstest]
fn test_execute_withdraw_respects_cooldown(mut vault: TestVault) {
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();
    vault.request_withdraw(user, user, 1000, 0, 0).unwrap();

    let early = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS - 1);
    assert!(early.is_err());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(!vault.state().unwrap().withdraw_queue.is_empty());

    let ok = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1);
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

    vault
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();

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

    let result = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1);
    assert!(result.is_ok());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
}

#[rstest]
fn test_happy_path_like_near_sequence(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let user = user_addr();
    let allocator = allocator_addr();

    vault
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();

    vault.deposit(user, user, 10_000, 0, 100).unwrap();

    vault.request_withdraw(user, user, 2_000, 0, 101).unwrap();
    vault
        .execute_withdraw(user, 101 + SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1)
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
        .execute_withdraw(user, 400 + SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1)
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
        .execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1)
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
    let user2 = Address([20u8; 32]);

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
    let user2 = Address([20u8; 32]);
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

    vault
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();
    vault
        .policy_state_mut()
        .set_market_config(1, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();
    vault
        .policy_state_mut()
        .set_market_config(2, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();

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
fn test_allocate_withdraw_uses_allocation_lifecycle(mut vault: TestVault) {
    use templar_soroban_runtime::contract::{AllocationDelta, Delta};

    let user = user_addr();
    let allocator = allocator_addr();

    vault
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();

    vault.deposit(user, user, 10_000, 0, 100).unwrap();
    let supply_result = vault
        .allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: 6_000,
            }),
        )
        .unwrap();

    assert_eq!(supply_result.op_id, 0);
    assert_eq!(vault.state().unwrap().next_op_id, 1);

    let withdraw_result = vault
        .allocate(
            allocator,
            &AllocationDelta::Withdraw(Delta {
                market: 0,
                amount: 2_000,
            }),
        )
        .unwrap();

    assert_eq!(withdraw_result.op_id, 1);
    assert_eq!(withdraw_result.new_external_assets, 4_000);
    assert!(vault.state().unwrap().op_state.is_idle());
    assert_eq!(vault.state().unwrap().next_op_id, 2);
    assert_eq!(vault.state().unwrap().idle_assets, 6_000);
    assert_eq!(vault.state().unwrap().external_assets, 4_000);
    assert_eq!(vault.state().unwrap().total_assets, 10_000);
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
    begin_allocating(&mut vault, allocator, vec![(0, 5000)], 1000).unwrap();

    // Try to start second allocation - should fail
    let result = begin_allocating(&mut vault, allocator, vec![(1, 3000)], 1000);
    assert!(result.is_err());
}

#[rstest]
fn test_cannot_refresh_while_allocating(mut vault: TestVault) {
    let user = user_addr();
    let allocator = allocator_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Start allocation
    begin_allocating(&mut vault, allocator, vec![(0, 5000)], 1000).unwrap();

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
    let result = begin_allocating(&mut vault, allocator, vec![(0, 5000)], 1000);
    assert!(result.is_err());
}

#[fixture]
fn dummy_ctx() -> EffectContext {
    EffectContext::new(
        0,
        Address([1u8; 32]),
        Address([2u8; 32]),
        Address([3u8; 32]),
    )
}

#[fixture]
fn mock_interpreter() -> MockInterpreter {
    MockInterpreter::new()
}

#[rstest]
fn test_deposit_effects_execute(mut mock_interpreter: MockInterpreter, dummy_ctx: EffectContext) {
    let effects = vec![
        KernelEffect::MintShares {
            owner: Address([9u8; 32]),
            shares: 100,
        },
        KernelEffect::EmitEvent {
            event: templar_vault_kernel::effects::KernelEvent::DepositProcessed {
                owner: Address([8u8; 32]),
                receiver: Address([9u8; 32]),
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
    let plan = vec![
        AllocationPlanEntry::new(0u32, 100u128),
        AllocationPlanEntry::new(1u32, 200u128),
    ];

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
        request_id: 7,
        amount: 150,
        receiver: Address([6u8; 32]),
        owner: Address([5u8; 32]),
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
#[rstest]
fn soroban_contract_deposit_after_donation_cannot_capture_surplus() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&curator, &contract_id, &(0u64)),
    );
    let asset_admin = soroban_sdk::Address::generate(&env);
    let asset_sac = env.register_stellar_asset_contract_v2(asset_admin.clone());
    let asset_token = asset_sac.address();
    let share_sac = env.register_stellar_asset_contract_v2(contract_id.clone());
    let share_token = share_sac.address();
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);
    let depositor = soroban_sdk::Address::generate(&env);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(
            env.clone(),
            curator.clone(),
            governance.clone(),
            asset_token.clone(),
            share_token.clone(),
            0,
            0,
        )
        .unwrap();

        let mut storage = SorobanStorage::new(&env);
        storage
            .save_state(&VaultState {
                total_assets: 500,
                total_shares: 500,
                idle_assets: 500,
                fee_anchor: FeeAccrualAnchor::new(500, templar_vault_kernel::TimestampNs(0)),
                ..Default::default()
            })
            .expect("save state");
    });

    asset_admin_client.mint(&contract_id, &500);
    asset_admin_client.mint(&contract_id, &300);
    asset_admin_client.mint(&depositor, &100);

    let proxy = VaultProxy::new(&env);
    let minted = env.as_contract(&contract_id, || {
        proxy
            .execute(&VaultCommand::DepositWithMin {
                owner: sdk_wire(&depositor),
                receiver: sdk_wire(&depositor),
                assets: 100,
                min_shares_out: 0,
            })
            .unwrap()
    });

    let minted_shares = DepositReceipt::decode(&minted.to_alloc_vec())
        .expect("deposit should return a deposit receipt")
        .shares_out;

    assert!(
        (1..=62).contains(&minted_shares),
        "deposit minted {minted_shares} shares outside expected post-donation bounds"
    );
}

#[rstest]
fn soroban_contract_resync_idle_balance_fixes_donation_accounting() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&curator, &contract_id, &(0u64)),
    );
    let asset_admin = soroban_sdk::Address::generate(&env);
    let asset_sac = env.register_stellar_asset_contract_v2(asset_admin.clone());
    let asset_token = asset_sac.address();
    let share_sac = env.register_stellar_asset_contract_v2(contract_id.clone());
    let share_token = share_sac.address();
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(
            env.clone(),
            governance.clone(),
            governance.clone(),
            asset_token.clone(),
            share_token.clone(),
            0,
            0,
        )
        .unwrap();

        let mut storage = SorobanStorage::new(&env);
        let state = VaultState {
            total_assets: 500,
            total_shares: 500,
            idle_assets: 500,
            fee_anchor: FeeAccrualAnchor::new(500, templar_vault_kernel::TimestampNs(0)),
            ..Default::default()
        };
        storage.save_state(&state).expect("save state");
    });

    asset_admin_client.mint(&contract_id, &500);

    let proxy = VaultProxy::new(&env);
    let (total_shares, idle_assets, external_assets) =
        env.as_contract(&contract_id, || proxy.snapshot().unwrap());
    assert_eq!(idle_assets, 500);
    assert_eq!(total_shares, 500);
    assert_eq!(external_assets, 0);

    let total_assets_before = env.as_contract(&contract_id, || proxy.total_assets().unwrap());
    assert_eq!(total_assets_before, 500);

    asset_admin_client.mint(&contract_id, &300);

    let actual_balance = soroban_sdk::token::Client::new(&env, &asset_token).balance(&contract_id);
    assert_eq!(actual_balance, 800);

    let total_assets_after_donation =
        env.as_contract(&contract_id, || proxy.total_assets().unwrap());
    let (total_shares_after, idle_assets_after, external_assets_after) =
        env.as_contract(&contract_id, || proxy.snapshot().unwrap());
    assert_eq!(total_assets_after_donation, 800);
    assert_eq!(idle_assets_after, 800);
    assert_eq!(total_shares_after, 500);
    assert_eq!(external_assets_after, 0);
    env.as_contract(&contract_id, || {
        let stored_state = SorobanStorage::new(&env)
            .load_state()
            .expect("load state")
            .expect("state present");
        assert_eq!(stored_state.total_assets, 500);
        assert_eq!(stored_state.idle_assets, 500);
    });

    env.as_contract(&contract_id, || {
        proxy
            .execute_unit(&VaultCommand::ResyncIdleBalance)
            .unwrap();
    });

    let total_assets_final = env.as_contract(&contract_id, || proxy.total_assets().unwrap());
    let (total_shares_final, idle_assets_final, external_assets_final) =
        env.as_contract(&contract_id, || proxy.snapshot().unwrap());
    assert_eq!(total_assets_final, 800);
    assert_eq!(idle_assets_final, 800);
    assert_eq!(total_shares_final, 500);
    assert_eq!(external_assets_final, 0);
    env.as_contract(&contract_id, || {
        let stored_state = SorobanStorage::new(&env)
            .load_state()
            .expect("load state")
            .expect("state present");
        assert_eq!(stored_state.total_assets, 800);
        assert_eq!(stored_state.idle_assets, 800);
        // Reconciliation raises the checkpoint by exactly the +300 inflow and never advances time.
        assert_eq!(stored_state.fee_anchor.total_assets, 800);
        assert_eq!(
            stored_state.fee_anchor.timestamp_ns,
            templar_vault_kernel::TimestampNs(0)
        );
    });
}

#[rstest]
fn soroban_contract_resync_idle_balance_crystallizes_fees_before_reconciliation() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&curator, &contract_id, &(0u64)),
    );
    let management_recipient = soroban_sdk::Address::generate(&env);
    let performance_recipient = soroban_sdk::Address::generate(&env);
    let asset_admin = soroban_sdk::Address::generate(&env);
    let asset_sac = env.register_stellar_asset_contract_v2(asset_admin.clone());
    let asset_token = asset_sac.address();
    let share_sac = env.register_stellar_asset_contract_v2(contract_id.clone());
    let share_token = share_sac.address();
    let asset_admin_client = StellarAssetClient::new(&env, &asset_token);
    let share_client = soroban_sdk::token::Client::new(&env, &share_token);
    let proxy = VaultProxy::new(&env);

    const STARTING_ASSETS: u128 = 1_000_000_000_000;
    const DONATED_ASSETS: u128 = 100_000_000_000;
    const RESYNC_NS: u64 = 100_000_000_000;
    const REFRESH_NS: u64 = 200_000_000_000;
    let refreshed_assets = STARTING_ASSETS + DONATED_ASSETS;
    let management_fee_wad = Wad::one() / 20;

    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(
            env.clone(),
            curator.clone(),
            governance.clone(),
            asset_token.clone(),
            share_token.clone(),
            0,
            0,
        )
        .unwrap();

        let mut storage = SorobanStorage::new(&env);
        storage
            .save_state(&VaultState {
                total_assets: STARTING_ASSETS,
                total_shares: STARTING_ASSETS,
                idle_assets: STARTING_ASSETS,
                fee_anchor: FeeAccrualAnchor::new(
                    STARTING_ASSETS,
                    templar_vault_kernel::TimestampNs(0),
                ),
                ..Default::default()
            })
            .expect("save state");
    });

    asset_admin_client.mint(&contract_id, &(refreshed_assets as i128));

    // Pre-crystallization (the ENG-700 fix) must mint the [0, RESYNC_NS] window before capital flow.
    let crystallized_management_shares = compute_management_fee_shares(
        STARTING_ASSETS,
        STARTING_ASSETS,
        STARTING_ASSETS,
        management_fee_wad,
        0,
        RESYNC_NS,
    )
    .as_u128_saturating();
    assert!(crystallized_management_shares > 0);
    env.as_contract(&contract_id, || {
        set_fee_policy(
            &env,
            &governance,
            &performance_recipient,
            &management_recipient,
            0,
        );
        proxy
            .execute_unit(&VaultCommand::ResyncIdleBalance)
            .unwrap();

        let stored_state = SorobanStorage::new(&env)
            .load_state()
            .expect("load state")
            .expect("state present");
        // The erased [T0, RESYNC_NS] window is minted at resync time under the stored checkpoint.
        assert_eq!(
            share_client.balance(&management_recipient),
            crystallized_management_shares as i128
        );
        assert_eq!(
            stored_state.total_shares,
            STARTING_ASSETS + crystallized_management_shares
        );
        // Reconciliation then credits the surplus and raises the checkpoint by exactly that inflow.
        assert_eq!(stored_state.idle_assets, refreshed_assets);
        assert_eq!(stored_state.total_assets, refreshed_assets);
        assert_eq!(
            stored_state.fee_anchor,
            FeeAccrualAnchor::new(
                refreshed_assets,
                templar_vault_kernel::TimestampNs(RESYNC_NS)
            )
        );
    });

    env.ledger().set(LedgerInfo {
        timestamp: 200,
        protocol_version: 25,
        ..Default::default()
    });

    let expected_management_shares = compute_management_fee_shares(
        refreshed_assets,
        refreshed_assets,
        STARTING_ASSETS + crystallized_management_shares,
        management_fee_wad,
        RESYNC_NS,
        REFRESH_NS,
    )
    .as_u128_saturating();
    assert!(expected_management_shares > 0);

    env.as_contract(&contract_id, || {
        proxy.execute_unit(&VaultCommand::RefreshFees).unwrap();

        let stored_state = SorobanStorage::new(&env)
            .load_state()
            .expect("load state")
            .expect("state present");
        assert_eq!(
            stored_state.total_shares,
            STARTING_ASSETS + crystallized_management_shares + expected_management_shares
        );
        assert_eq!(
            share_client.balance(&management_recipient),
            (crystallized_management_shares + expected_management_shares) as i128
        );
        assert_eq!(share_client.balance(&performance_recipient), 0);
        assert_eq!(
            stored_state.fee_anchor,
            FeeAccrualAnchor::new(
                refreshed_assets,
                templar_vault_kernel::TimestampNs(REFRESH_NS)
            )
        );
    });
}

const ENG700_ASSETS: u128 = 1_000_000_000_000;
const ENG700_FEE_ANCHOR: u128 = 900_000_000_000;
const ENG700_ZERO_FEE_ANCHOR: u128 = 1_100_000_000_000;
const ENG700_DONATION: u128 = 100_000_000_000;
const ENG700_SHORTFALL: u128 = 1_000_000;
const ENG700_LEDGER_SECONDS: u64 = 63_072_000;
const ENG700_LEDGER_NS: u64 = ENG700_LEDGER_SECONDS * 1_000_000_000;
const ENG700_RECOVERY_SECONDS: u64 = 126_144_000;
const ENG700_RECOVERY_NS: u64 = ENG700_RECOVERY_SECONDS * 1_000_000_000;
const ENG700_DEPOSIT_ASSETS: i128 = 100;
const ENG700_DONATION_DELTA: i128 = ENG700_DONATION as i128;
const ENG700_SHORTFALL_DELTA: i128 = -(ENG700_SHORTFALL as i128);

// Configure the fee legs through the governance bridge, or clear them for the zero-fee rows.
// Callers must already be inside the vault contract context.
fn set_fee_policy(
    env: &Env,
    governance: &soroban_sdk::Address,
    performance_recipient: &soroban_sdk::Address,
    management_recipient: &soroban_sdk::Address,
    fee_mode: u8,
) {
    let (performance_fee_wad, management_fee_wad) = match fee_mode {
        0 => (Wad::zero(), Wad::one() / 20),
        1 => (Wad::one() / 5, Wad::zero()),
        2 => (Wad::one() / 5, Wad::one() / 20),
        _ => (Wad::zero(), Wad::zero()),
    };
    VaultProxy::new(env)
        .execute_governance_unit(
            governance,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_FEES,
                target_ids: None,
                mode: None,
                accounts: Some(vec![
                    sdk_wire(performance_recipient),
                    sdk_wire(management_recipient),
                ]),
                market_id: None,
                cap_group_id: None,
                value: Some(performance_fee_wad.as_u128_trunc() as i128),
                value_b: Some(management_fee_wad.as_u128_trunc() as i128),
                value_c: None,
            },
        )
        .expect("configure fees");
}

struct Eng700Fixture {
    env: Env,
    contract_id: soroban_sdk::Address,
    asset_token: soroban_sdk::Address,
    share_token: soroban_sdk::Address,
    management_recipient: soroban_sdk::Address,
    performance_recipient: soroban_sdk::Address,
    depositor: soroban_sdk::Address,
}

// fee_mode: 0 = management only, 1 = performance only, 2 = both, 3 = zero fees.
fn eng700_fixture(fee_mode: u8) -> Eng700Fixture {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SorobanVaultContract, ());
    let curator = soroban_sdk::Address::generate(&env);
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&curator, &contract_id, &(0u64)),
    );
    let asset_token = env
        .register_stellar_asset_contract_v2(soroban_sdk::Address::generate(&env))
        .address();
    let share_token = env
        .register_stellar_asset_contract_v2(contract_id.clone())
        .address();
    let management_recipient = soroban_sdk::Address::generate(&env);
    let performance_recipient = soroban_sdk::Address::generate(&env);
    let depositor = soroban_sdk::Address::generate(&env);
    env.as_contract(&contract_id, || {
        SorobanVaultContract::initialize(
            env.clone(),
            governance.clone(),
            governance.clone(),
            asset_token.clone(),
            share_token.clone(),
            0,
            0,
        )
        .expect("initialize");
        SorobanStorage::new(&env)
            .save_state(&VaultState {
                total_assets: ENG700_ASSETS,
                total_shares: ENG700_ASSETS,
                idle_assets: ENG700_ASSETS,
                fee_anchor: FeeAccrualAnchor::new(
                    if fee_mode == 3 {
                        ENG700_ZERO_FEE_ANCHOR
                    } else {
                        ENG700_FEE_ANCHOR
                    },
                    templar_vault_kernel::TimestampNs(0),
                ),
                ..Default::default()
            })
            .expect("save state");
        if fee_mode != 3 {
            set_fee_policy(
                &env,
                &governance,
                &performance_recipient,
                &management_recipient,
                fee_mode,
            );
        }
    });
    Eng700Fixture {
        env,
        contract_id,
        asset_token,
        share_token,
        management_recipient,
        performance_recipient,
        depositor,
    }
}

fn eng700_set_ledger(fixture: &Eng700Fixture, timestamp: u64) {
    fixture.env.ledger().set(LedgerInfo {
        timestamp,
        protocol_version: 25,
        ..Default::default()
    });
}

// trigger: 0 = RefreshFees, 1 = DepositWithMin, 2 = ResyncIdleBalance.
fn eng700_run_trigger(fixture: &Eng700Fixture, trigger: u8) -> i128 {
    let proxy = VaultProxy::new(&fixture.env);
    let mut deposit_shares_out = 0i128;
    fixture
        .env
        .as_contract(&fixture.contract_id, || match trigger {
            0 => {
                proxy
                    .execute_unit(&VaultCommand::RefreshFees)
                    .expect("refresh fees trigger");
            }
            2 => {
                proxy
                    .execute_unit(&VaultCommand::ResyncIdleBalance)
                    .expect("resync trigger");
            }
            _ => {
                let receipt = proxy
                    .execute(&VaultCommand::DepositWithMin {
                        owner: sdk_wire(&fixture.depositor),
                        receiver: sdk_wire(&fixture.depositor),
                        assets: ENG700_DEPOSIT_ASSETS,
                        min_shares_out: 0,
                    })
                    .expect("deposit trigger");
                deposit_shares_out = DepositReceipt::decode(&receipt.to_alloc_vec())
                    .expect("deposit receipt")
                    .shares_out;
            }
        });
    deposit_shares_out
}

// The stored VaultState plus the vault asset, management recipient, performance
// recipient, and depositor share balances.
fn eng700_observe(fixture: &Eng700Fixture) -> (VaultState, i128, i128, i128, i128) {
    let stored_state = fixture.env.as_contract(&fixture.contract_id, || {
        SorobanStorage::new(&fixture.env)
            .load_state()
            .expect("load state")
            .expect("state present")
    });
    let share_client = soroban_sdk::token::Client::new(&fixture.env, &fixture.share_token);
    (
        stored_state,
        soroban_sdk::token::Client::new(&fixture.env, &fixture.asset_token)
            .balance(&fixture.contract_id),
        share_client.balance(&fixture.management_recipient),
        share_client.balance(&fixture.performance_recipient),
        share_client.balance(&fixture.depositor),
    )
}

// A configured leg must be strictly positive and a disabled leg strictly zero;
// every minted share must reach a configured recipient or a deposit receiver.
fn eng700_assert_fees(
    fee_mode: u8,
    total_shares: i128,
    management: i128,
    performance: i128,
    deposit_shares: i128,
) {
    let management_configured = matches!(fee_mode, 0 | 2);
    let performance_configured = matches!(fee_mode, 1 | 2);
    assert_eq!(
        (management > 0, management == 0),
        (management_configured, !management_configured),
        "management leg must mint exactly when configured"
    );
    assert_eq!(
        (performance > 0, performance == 0),
        (performance_configured, !performance_configured),
        "performance leg must mint exactly when configured"
    );
    assert_eq!(
        total_shares,
        ENG700_ASSETS as i128 + management + performance + deposit_shares,
        "supply must equal the seeded supply plus crystallized and purchased shares only"
    );
}

// One trigger step against the given fixture: fund the vault at the checkpoint
// plus delta, crystallize elapsed fees, execute the trigger, reconcile the live
// balance, and pin the fee legs, restated assets, and checkpoint.
fn eng700_step(
    fixture: &Eng700Fixture,
    trigger: u8,
    fee_mode: u8,
    delta: i128,
) -> (VaultState, i128, i128, i128, i128) {
    StellarAssetClient::new(&fixture.env, &fixture.asset_token)
        .mint(&fixture.contract_id, &(ENG700_ASSETS as i128 + delta));
    if trigger == 1 {
        StellarAssetClient::new(&fixture.env, &fixture.asset_token)
            .mint(&fixture.depositor, &ENG700_DEPOSIT_ASSETS);
    }
    eng700_set_ledger(&fixture, ENG700_LEDGER_SECONDS);
    let deposit_shares = eng700_run_trigger(&fixture, trigger);
    let (stored, vault_balance, management, performance, depositor) = eng700_observe(fixture);
    assert_eq!(deposit_shares > 0, trigger == 1);
    assert_eq!(depositor, deposit_shares);
    eng700_assert_fees(
        fee_mode,
        stored.total_shares as i128,
        management,
        performance,
        deposit_shares,
    );
    let deposit_inflow = if trigger == 1 {
        ENG700_DEPOSIT_ASSETS
    } else {
        0
    };
    let restated = ENG700_ASSETS as i128 + deposit_inflow + delta;
    assert_eq!(
        (
            stored.idle_assets as i128,
            stored.total_assets as i128,
            vault_balance
        ),
        (restated, restated, restated)
    );
    // Crystallization moves the checkpoint to the recorded total at trigger time, and a deposit
    // rewrites it to the post-deposit total. With fees unconfigured, only a matched-balance fee
    // refresh re-anchors; reconciliation itself adds a positive inflow on top of the checkpoint
    // and preserves its timestamp, and never lowers the checkpoint for an outflow.
    let re_anchored = fee_mode != 3 || trigger == 1 || (delta == 0 && trigger == 0);
    let checkpoint = if trigger == 1 {
        ENG700_ASSETS as i128 + deposit_inflow
    } else if re_anchored {
        ENG700_ASSETS as i128
    } else {
        ENG700_ZERO_FEE_ANCHOR as i128
    };
    assert_eq!(
        stored.fee_anchor,
        FeeAccrualAnchor::new(
            (checkpoint + delta.max(0)) as u128,
            templar_vault_kernel::TimestampNs(if re_anchored { ENG700_LEDGER_NS } else { 0 }),
        ),
    );
    (stored, vault_balance, management, performance, depositor)
}

fn eng700_check(trigger: u8, fee_mode: u8, delta: i128) -> (VaultState, i128, i128, i128, i128) {
    let fixture = eng700_fixture(fee_mode);
    eng700_step(&fixture, trigger, fee_mode, delta)
}

#[rstest]
fn soroban_contract_eng700_triggers_crystallize_before_balance_actions(
    #[values(0u8, 1u8, 2u8)] trigger: u8,
    #[values(0u8, 1u8, 2u8, 3u8)] fee_mode: u8,
) {
    eng700_check(trigger, fee_mode, 0);
}

#[rstest]
fn soroban_contract_eng700_mismatch_reconciles_as_capital_flow(
    #[values(0u8, 2u8)] trigger: u8,
    #[values(0u8, 1u8, 2u8, 3u8)] fee_mode: u8,
    #[values(ENG700_DONATION_DELTA, ENG700_SHORTFALL_DELTA)] delta: i128,
) {
    eng700_check(trigger, fee_mode, delta);
}

// Paired undisturbed and one-unit-donation runs must crystallize identical fee
// legs and mint identical deposit shares, because crystallization and share
// pricing run against the persisted accounting before reconciliation credits the
// inflow, and a one-unit base cannot move any truncated leg or purchase. Zero
// rounding tolerance: a leg that is zeroed by reconciliation-first handling or
// whole-anchor replacement, or deposit shares minted against reconciled
// accounting, trips these exact balance comparisons.
#[rstest]
fn soroban_contract_eng700_one_unit_donation_preserves_crystallized_fees_and_deposit_shares(
    #[values(0u8, 1u8)] trigger: u8,
    #[values(0u8, 1u8, 2u8)] fee_mode: u8,
) {
    let (.., control_management, control_performance, control_depositor) =
        eng700_check(trigger, fee_mode, 0);
    let (.., donation_management, donation_performance, donation_depositor) =
        eng700_check(trigger, fee_mode, 1);
    assert_eq!(
        donation_management, control_management,
        "a donated unit must not alter the management fee leg"
    );
    assert_eq!(
        donation_performance, control_performance,
        "a donated unit must not alter the performance fee leg"
    );
    assert_eq!(
        donation_depositor, control_depositor,
        "deposit shares must not be minted against reconciled accounting"
    );
}

#[rstest]
fn soroban_contract_eng700_loss_then_recovery_resyncs_keep_checkpoint_and_charge_zero_recovery_fees(
) {
    let fixture = eng700_fixture(2);
    // The checkpoint follows the pre-loss accounting, so the loss is not recoverable profit.
    let (.., lost_performance, _) = eng700_step(&fixture, 2, 2, -ENG700_DONATION_DELTA);

    StellarAssetClient::new(&fixture.env, &fixture.asset_token)
        .mint(&fixture.contract_id, &ENG700_DONATION_DELTA);
    eng700_set_ledger(&fixture, ENG700_RECOVERY_SECONDS);
    eng700_run_trigger(&fixture, 2);
    let (recovered, recovered_balance, recovered_management, recovered_performance, _) =
        eng700_observe(&fixture);
    assert_eq!(
        recovered_performance, lost_performance,
        "recovery inflow must not be charged as performance profit"
    );
    eng700_assert_fees(
        2,
        recovered.total_shares as i128,
        recovered_management,
        recovered_performance,
        0,
    );
    let restored = ENG700_ASSETS as i128;
    assert_eq!(
        (
            recovered.idle_assets as i128,
            recovered.total_assets as i128,
            recovered_balance
        ),
        (restored, restored, restored)
    );
    assert_eq!(
        recovered.fee_anchor,
        FeeAccrualAnchor::new(
            ENG700_ASSETS,
            templar_vault_kernel::TimestampNs(ENG700_RECOVERY_NS)
        )
    );
}
