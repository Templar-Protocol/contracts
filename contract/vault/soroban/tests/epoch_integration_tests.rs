//! Integration tests for the Soroban curator vault.
//!
//! These tests verify full flows: deposit -> allocate -> refresh -> withdraw.
//!
//! ENG-697 settlement-anchored law suite: drives immediate entrypoints and
//! epoch settlement together; compiled only when both features are enabled.

#![cfg(all(feature = "epoch", feature = "immediate-entrypoints"))]

use rstest::{fixture, rstest};
use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    token::StellarAssetClient,
    Address as SdkAddress, Bytes, Env,
};
use std::string::String as AllocString;
use templar_curator_primitives::policy::state::MarketConfig;
use templar_soroban_governance::{GovernanceError, SorobanVaultGovernanceContract};
use templar_soroban_runtime::{
    contract::{
        ContractConfig, CuratorVault, ExecuteWithdrawResult, SorobanVaultContract,
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
    BeginEpochCutoffReceipt, CancelPendingWithdrawalReceipt, ConfigureEpochSettlementReceipt,
    DepositReceipt, EmptyReceipt, EpochStateViewReceipt, ExecuteWithdrawReceipt,
    GovernanceCommand, RequestWithdrawReceipt, VaultCommand, EPOCH_PHASE_CUTOFF,
    GOVERNANCE_CONFIG_KIND_ALLOCATORS,
    GOVERNANCE_CONFIG_KIND_CURATOR, GOVERNANCE_CONFIG_KIND_SENTINEL,
    GOVERNANCE_CONFIG_KIND_VIRTUAL_OFFSETS, GOVERNANCE_POLICY_KIND_CAP,
    GOVERNANCE_POLICY_KIND_FEES, GOVERNANCE_POLICY_KIND_PAUSED, GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
};
use templar_vault_kernel::{
    apply_action, compute_fee_shares_from_assets, compute_management_fee_shares,
    effects::{KernelEffect, KernelEvent}, total_assets_for_fee_accrual, Address, AllocatingState,
    AllocationPlanEntry, FeeAccrualAnchor, FeeSlot, FeesSpec, KernelAction, Number, OpState,
    Restrictions, VaultConfig, VaultState, Wad, MAX_PENDING, MIN_WITHDRAWAL_ASSETS,
};
use templar_vault_kernel::{
    state::op_state::RefreshingState,
    transitions::{
        allocation_step_callback, complete_allocation, complete_refresh, payout_complete,
        refresh_step_callback, start_allocation, start_refresh, start_withdrawal,
        withdrawal_settled, withdrawal_step_callback, WithdrawalRequest,
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

    if state.total_shares == 0 || now_ns <= anchor.timestamp_ns.as_u64() {
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

/// Close intake at `cutoff_ns` and settle the epoch through the kernel's
/// settlement law, then persist the settled state. Claims in the calling
/// test are derived only from the resulting immutable snapshot; nothing in
/// these tests stores or pays a request-time figure.
fn cut_off_and_settle_epoch(vault: &mut TestVault, cutoff_ns: u64, reported_external_assets: u128) {
    use templar_vault_kernel::state::settlement::ValuationReportRef;
    use templar_vault_kernel::TimestampNs;

    let self_id = test_config().vault_address;
    let config = preview_kernel_config(false, 0, 0);
    let before = vault.state().expect("state is loaded").clone();
    let report_seq = before
        .epoch
        .last_settled
        .as_ref()
        .map_or(1, |snapshot| snapshot.report_seq() + 1);
    let cutoff = apply_action(
        before,
        &config,
        None,
        &self_id,
        KernelAction::BeginEpochCutoff {
            cutoff_ns: TimestampNs(cutoff_ns),
            now_ns: TimestampNs(cutoff_ns),
        },
    )
    .expect("epoch cutoff accepted");
    let settled = apply_action(
        cutoff.state,
        &config,
        None,
        &self_id,
        KernelAction::SettleEpoch {
            report: ValuationReportRef {
                report_seq,
                as_of_ns: TimestampNs(cutoff_ns.saturating_add(1)),
                report_hash: [7u8; 32],
            },
            new_external_assets: reported_external_assets,
            max_report_age_ns: SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS,
            settle_now_ns: TimestampNs(cutoff_ns.saturating_add(2)),
        },
    )
    .expect("epoch settlement accepted");
    *vault.state_mut().expect("state is loaded") = settled.state;
    vault.save_state().expect("settled epoch persists");
    let after = vault.state().expect("state is loaded");
    assert!(after.check_invariant());
    assert!(after.epoch.last_settled.is_some());
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
        timestamp: 100,
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
    let request_id = env.as_contract(&contract_id, || {
        proxy
            .execute(&VaultCommand::DepositWithMin {
                owner: sdk_wire(&owner),
                receiver: sdk_wire(&owner),
                assets: deposit_assets,
                min_shares_out: 0,
            })
            .unwrap();
        let withdrawal = proxy
            .execute(&VaultCommand::RequestWithdraw {
                owner: sdk_wire(&owner),
                receiver: sdk_wire(&owner),
                shares: deposit_assets,
                min_assets_out: 0,
            })
            .unwrap();
        let withdrawal = RequestWithdrawReceipt::decode(&withdrawal.to_alloc_vec())
            .expect("decode withdrawal receipt");
        assert_eq!(withdrawal.shares_escrowed, deposit_assets);
        let request_id = withdrawal.request_id;
        let queued = SorobanStorage::new(&env)
            .load_state()
            .expect("load queued state")
            .expect("queued state present");
        let (persisted_id, pending) = queued
            .withdraw_queue
            .head()
            .expect("persisted withdrawal");
        assert_eq!(persisted_id, request_id);
        assert_eq!(pending.escrow_shares, deposit_assets as u128);
        persisted_id
    });

    env.ledger().set(LedgerInfo {
        timestamp: SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS / 1_000_000_000 + 2,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&contract_id, || {
        let governance = proxy.governance().unwrap();
        let config_receipt = proxy
            .execute(&VaultCommand::ConfigureEpochSettlement {
                caller: sdk_wire(&governance),
                max_report_age_ns: 60_000_000_000,
            })
            .unwrap();
        assert_eq!(
            ConfigureEpochSettlementReceipt::decode(&config_receipt.to_alloc_vec())
                .expect("decode config receipt"),
            ConfigureEpochSettlementReceipt {
                max_report_age_ns: 60_000_000_000,
            }
        );
        let cutoff_receipt = proxy
            .execute(&VaultCommand::BeginEpochCutoff {
                caller: sdk_wire(&curator),
                cutoff_ns: (SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS / 1_000_000_000 + 2)
                    * 1_000_000_000,
            })
            .unwrap();
        assert_eq!(
            BeginEpochCutoffReceipt::decode(&cutoff_receipt.to_alloc_vec())
                .expect("decode cutoff receipt"),
            BeginEpochCutoffReceipt {
                epoch_id: 1,
                cutoff_ns: (SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS / 1_000_000_000 + 2)
                    * 1_000_000_000,
            }
        );
        // Current accepted law requires an authenticated, accepted custodial
        // report after cutoff before any settlement can proceed. With none,
        // settlement halts fail-closed, exposes no receipts, and never
        // progresses the epoch.
        let settle_err = proxy
            .execute(&VaultCommand::SettleEpoch {
                caller: sdk_wire(&curator),
            })
            .expect_err("settlement without an accepted custodial report must halt");
        // Fail-closed law: settlement is refused before any progression or
        // payout. The refusal may surface as InvalidState when no authenticated
        // adapter metadata exists, or as a storage integrity halt when prior
        // queue state contradicts epoch liability. Both are lawful fail-closed
        // refusals; the epoch-progression, leak, and refund invariants asserted
        // below are the binding law.
        assert!(matches!(
            settle_err,
            templar_soroban_runtime::ContractError::InvalidState
                | templar_soroban_runtime::ContractError::StorageError
        ));
        // The epoch must not progress while settlement is halted without an
        // accepted custodial report.
        let view_halted = EpochStateViewReceipt::decode(
            &proxy
                .execute(&VaultCommand::GetEpochState)
                .expect("epoch state view")
                .to_alloc_vec(),
        )
        .expect("decode epoch state view");
        assert_eq!(view_halted.phase, EPOCH_PHASE_CUTOFF);
        assert_eq!(view_halted.last_settled_epoch_id, None);

        // The halted settlement must not leak a completed payout: the
        // withdrawal queue stays intact and no assets leave the vault.
        let state_halted = SorobanStorage::new(&env)
            .load_state()
            .expect("load state")
            .expect("state present");
        assert!(!state_halted.withdraw_queue.is_empty());
        assert_eq!(
            asset_admin_client.balance(&contract_id),
            deposit_assets,
            "halted settlement must not move custodied assets"
        );
        // A stranger cannot claim someone else's escrow: non-owner
        // cancellation of the real persisted request is refused and the
        // queue and escrow stay intact.
        let stranger = soroban_sdk::Address::generate(&env);
        let stranger_err = proxy
            .execute(&VaultCommand::CancelPendingWithdrawal {
                owner: sdk_wire(&stranger),
                request_id,
            })
            .expect_err("non-owner cancellation must be refused");
        assert_eq!(
            stranger_err,
            templar_soroban_runtime::ContractError::Unauthorized
        );
        let state_after_stranger = SorobanStorage::new(&env)
            .load_state()
            .expect("load state")
            .expect("state present");
        let (head_after_stranger, _) = state_after_stranger
            .withdraw_queue
            .head()
            .expect("escrow must survive a refused non-owner cancellation");
        assert_eq!(head_after_stranger, request_id);
        assert_eq!(asset_admin_client.balance(&contract_id), deposit_assets);
        assert_eq!(asset_admin_client.balance(&stranger), 0);
    });

    // The owner-bound refund law still works while settlement is halted:
    // cancellation refunds the full escrow and the epoch stays unsettled.
    env.ledger().set(LedgerInfo {
        timestamp: SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS / 1_000_000_000 + 3,
        protocol_version: 25,
        ..Default::default()
    });
    let refund = env.as_contract(&contract_id, || {
        proxy
            .execute(&VaultCommand::CancelPendingWithdrawal {
                owner: sdk_wire(&owner),
                request_id,
            })
            .expect("owner-bound cancellation must refund the escrow")
    });
    let cancelled = CancelPendingWithdrawalReceipt::decode(&refund.to_alloc_vec())
        .expect("decode owner-bound cancellation receipt");
    assert_eq!(cancelled.request_id, request_id);
    assert_eq!(cancelled.shares_refunded, deposit_assets);
    assert_eq!(cancelled.epoch_id, 1);

    env.as_contract(&contract_id, || {
        let state_after = SorobanStorage::new(&env)
            .load_state()
            .unwrap()
            .expect("state should remain persisted");
        assert!(state_after.withdraw_queue.is_empty());
        let view_after = EpochStateViewReceipt::decode(
            &proxy
                .execute(&VaultCommand::GetEpochState)
                .expect("epoch state view")
                .to_alloc_vec(),
        )
        .expect("decode epoch state view");
        assert_eq!(view_after.phase, EPOCH_PHASE_CUTOFF);
        assert_eq!(view_after.last_settled_epoch_id, None);
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
}

#[fixture]
fn rbac_vault() -> RbacVault {
    create_rbac_vault()
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
fn test_withdraw_request_stores_no_request_time_quote(mut vault: TestVault) {
    let user = user_addr();

    // Deposit and establish share ratio
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Withdraw half the shares: the queue records only escrow shares, the
    // declared slippage floor, and the settlement epoch binding. No
    // asset-denominated claim may exist before an accepted settlement.
    let result = vault.request_withdraw(user, user, 5000, 4000, 200).unwrap();
    assert_eq!(result.shares_escrowed, 5000);
    let (head_id, pending) = vault
        .state()
        .unwrap()
        .withdraw_queue
        .head()
        .expect("pending withdrawal");
    assert_eq!(head_id, result.request_id);
    assert_eq!(pending.escrow_shares, 5000);
    assert_eq!(pending.min_assets_out, 4000);
    assert!(pending.requested_at_ns.as_u64() > 0);
    assert_eq!(
        pending.epoch_id,
        vault.state().unwrap().epoch.intake_epoch
    );
    // Pre-settlement intake carries no claim authority: the settled-epoch
    // view derives nothing until a snapshot is bound to its epoch.
    let settled_claim = vault
        .state()
        .unwrap()
        .epoch
        .settled_claim_for(pending.epoch_id, pending.escrow_shares);
    assert!(settled_claim.is_none());
    assert!(vault
        .state()
        .unwrap()
        .withdraw_queue
        .settled_head_claim(&vault.state().unwrap().epoch)
        .is_none());
}

#[rstest]
fn test_withdraw_request_slippage_protection(mut vault: TestVault) {
    use templar_vault_kernel::{KernelAction, TimestampNs};

    let user = user_addr();

    // Deposit
    vault.deposit(user, user, 10000, 0, 100).unwrap();

    // Settlement-anchored law has no request-time price bound: an
    // overpriced intake is accepted unpriced and carries no quote.
    vault
        .request_withdraw(user, user, 1000, 2000, 200)
        .expect("overpriced intake is accepted unpriced");

    // Settlement closes intake and binds an immutable snapshot.
    cut_off_and_settle_epoch(&mut vault, 50, 0);

    // The escrowed head is settled at 1000 assets. Its stored request
    // demands at least 2000 assets, so settlement halts at that head:
    // execution fails closed, the head is not skipped, escrow is intact,
    // and no other position is affected.
    let halted = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1);
    assert!(halted.is_err(), "settlement must halt at an unmet head");
    let halted_state = vault.state().unwrap();
    assert!(halted_state.op_state.is_idle());
    assert!(!halted_state.withdraw_queue.is_empty());
    assert_eq!(halted_state.idle_assets, 10000);
    assert_eq!(halted_state.total_assets, 10000);
    assert_eq!(halted_state.total_shares, 10000);
    let _ = halted_state;

    // The owner cancels the halted head and receives the full escrow back.
    let self_id = test_config().vault_address;
    let config = preview_kernel_config(false, 0, 0);
    let before = vault.state().unwrap().clone();
    let halted_head_id = before
        .withdraw_queue
        .iter()
        .next()
        .expect("halted head")
        .0;
    let cancelled = apply_action(
        before,
        &config,
        None,
        &self_id,
        KernelAction::CancelPendingWithdrawal {
            caller: user,
            request_id: halted_head_id,
            now_ns: TimestampNs(SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 2),
        },
    )
    .expect("owner cancels the halted head");
    let escrow_refunded = cancelled
        .effects
        .iter()
        .find_map(|effect| match effect {
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalCancelled { escrow_shares, .. },
            } => Some(*escrow_shares),
            _ => None,
        })
        .expect("cancellation binds escrow refund");
    assert_eq!(escrow_refunded, 1000, "cancellation refunds the full escrow");
    *vault.state_mut().unwrap() = cancelled.state;
    vault.save_state().unwrap();
    let after = vault.state().unwrap();
    assert!(after.withdraw_queue.is_empty(), "queue drains after refund");
    assert_eq!(after.idle_assets, 10000);
    assert_eq!(after.total_assets, 10000);
    assert_eq!(after.total_shares, 10000);
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
fn test_execute_withdraw_fails_closed_until_epoch_settled(mut vault: TestVault) {
    let user = user_addr();

    // Deposit
    vault.deposit(user, user, 10000, 0, 100).unwrap();
    vault.request_withdraw(user, user, 1000, 0, 0).unwrap();

    // Even in Idle state with the cooldown elapsed, an unsettled head has
    // no claim to pay: execution stops fail closed and nothing moves.
    let result = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1);
    assert!(result.is_err());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(!vault.state().unwrap().withdraw_queue.is_empty());
    assert_eq!(vault.state().unwrap().idle_assets, 10000);
    assert_eq!(vault.state().unwrap().total_assets, 10000);
    assert_eq!(vault.state().unwrap().total_shares, 10000);
}

#[rstest]
fn test_execute_withdraw_settles_only_after_epoch_and_cooldown(mut vault: TestVault) {
    let user = user_addr();

    vault.deposit(user, user, 10000, 0, 100).unwrap();
    vault.request_withdraw(user, user, 1000, 0, 0).unwrap();

    // Settlement first closes intake and binds an immutable snapshot.
    cut_off_and_settle_epoch(&mut vault, 50, 0);

    // The cooldown still gates progression after settlement.
    let early = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS - 1);
    assert!(early.is_err());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(!vault.state().unwrap().withdraw_queue.is_empty());

    // After the cooldown the settled claim is paid exactly once and the
    // escrowed shares are burned.
    let ok = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1);
    assert!(ok.is_ok());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
    assert_eq!(vault.state().unwrap().idle_assets, 9000);
    assert_eq!(vault.state().unwrap().total_assets, 9000);
    assert_eq!(vault.state().unwrap().total_shares, 9000);
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

    // The queued request is only an escrow of shares until the epoch
    // settles; execution must fail closed before settlement and can pay
    // only the claim derived from the bound snapshot afterwards.
    let unsettled = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1);
    assert!(unsettled.is_err());
    assert!(!vault.state().unwrap().withdraw_queue.is_empty());

    cut_off_and_settle_epoch(&mut vault, 500, 5000);

    let result = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1);
    assert!(result.is_ok());
    assert!(vault.state().unwrap().op_state.is_idle());
    assert!(vault.state().unwrap().withdraw_queue.is_empty());
    assert_eq!(vault.state().unwrap().idle_assets, 1000);
    assert_eq!(vault.state().unwrap().external_assets, 5000);
    assert_eq!(vault.state().unwrap().total_assets, 6000);
    assert_eq!(vault.state().unwrap().total_shares, 6000);
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

    // Queued intake is unpriced: the epoch must first close at a cutoff and
    // settle against a fresh accepted report. Execution then pays only the
    // claim derived from that immutable snapshot.
    vault.request_withdraw(user, user, 2_000, 0, 101).unwrap();
    cut_off_and_settle_epoch(&mut vault, 50, 0);
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
    cut_off_and_settle_epoch(&mut vault, 500, 9_000);
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

    // Until the epoch settles, the head has no derived claim and the whole
    // run must stop with nothing paid and nothing dequeued.
    let unsettled = vault.execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 2);
    assert!(unsettled.is_err());
    assert!(vault.state().unwrap().withdraw_queue.contains(first.request_id));

    cut_off_and_settle_epoch(&mut vault, 50, 0);

    vault
        .execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 2)
        .unwrap();

    assert!(!vault.state().unwrap().withdraw_queue.contains(first.request_id));
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
fn test_withdrawal_transition_flow_reaches_idle(mut vault: TestVault) {
    let user = user_addr();

    vault.deposit(user, user, 10_000, 0, 100).unwrap();
    let escrowed = vault
        .request_withdraw(user, user, 1_000, 0, 0)
        .unwrap()
        .shares_escrowed;
    assert_eq!(escrowed, 1_000);

    // Accepted settlement of the head's own epoch binds the only pricing
    // authority; the claim below is derived from that immutable snapshot.
    cut_off_and_settle_epoch(&mut vault, 50, 0);
    let state = vault.state().unwrap();
    let snapshot = state
        .epoch
        .last_settled
        .as_ref()
        .expect("accepted settlement snapshot");
    let claim = snapshot
        .claim_for(escrowed)
        .expect("derived settled claim");
    assert!(claim > 0);

    let result = vault
        .execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1)
        .expect("settled withdrawal executes");
    let ExecuteWithdrawResult::Payout { completed, .. } = result else {
        panic!("settled FIFO head must pay out");
    };
    assert_eq!(completed.assets_out, claim);
    assert_eq!(completed.shares_burned, escrowed);

    // The successful payout burns the full recorded escrow exactly once.
    let burns: Vec<u128> = vault
        .interpreter
        .effects
        .iter()
        .filter_map(|effect| match effect {
            KernelEffect::BurnShares { shares, .. } => Some(*shares),
            _ => None,
        })
        .collect();
    assert_eq!(burns, vec![escrowed]);
    assert!(vault.interpreter.effects.iter().any(|effect| {
        matches!(effect, KernelEffect::EmitEvent {
            event: KernelEvent::WithdrawalCollected { burn_shares, collected, .. },
        } if *burn_shares == escrowed && *collected == claim)
    }));
    assert!(vault.interpreter.effects.iter().any(|effect| {
        matches!(effect, KernelEffect::EmitEvent {
            event: KernelEvent::PayoutCompleted {
                success: true,
                burn_shares,
                refund_shares: 0,
                amount,
                ..
            },
        } if *burn_shares == escrowed && *amount == claim)
    }));
    assert!(vault.interpreter.effects.iter().any(|effect| {
        matches!(effect, KernelEffect::TransferAssets { amount, .. } if *amount == claim)
    }));
    // Nothing ever left the escrow account except the single lawful burn.
    let escrow_address = test_config().vault_address;
    assert!(!vault.interpreter.effects.iter().any(|effect| {
        matches!(effect, KernelEffect::TransferShares { from, .. } if *from == escrow_address)
    }));

    // Accounting reconciles exactly and the vault returns idle with an
    // empty queue; the owner retains claim on the only remaining supply.
    let after = vault.state().unwrap();
    assert_eq!(after.idle_assets + after.external_assets, after.total_assets);
    assert_eq!(after.total_assets, 10_000 - claim);
    assert_eq!(after.idle_assets, 10_000 - claim);
    assert_eq!(after.external_assets, 0);
    assert_eq!(after.total_shares, 10_000 - escrowed);
    assert!(after.withdraw_queue.head().is_none());
    assert!(after.op_state.is_idle());
    assert!(after.check_invariant());

    // A replay finds no queued head: no second burn and no payout.
    assert!(vault
        .execute_withdraw(user, SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 2)
        .is_err());
    let replayed = vault.state().unwrap();
    assert_eq!(replayed.total_assets, 10_000 - claim);
    assert_eq!(replayed.total_shares, 10_000 - escrowed);
    assert!(replayed.op_state.is_idle());
    assert_eq!(
        vault
            .interpreter
            .effects
            .iter()
            .filter(|effect| matches!(effect, KernelEffect::BurnShares { .. }))
            .count(),
        1
    );
}

#[rstest]
fn test_payout_transition_failure_refunds_full_escrow(mut vault: TestVault) {
    let user = user_addr();

    vault.deposit(user, user, 10_000, 0, 100).unwrap();
    let escrowed = vault
        .request_withdraw(user, user, 1_000, 0, 0)
        .unwrap()
        .shares_escrowed;

    // Accepted settlement binds the immutable snapshot for this head's
    // epoch; the payout claim is derived only from it.
    cut_off_and_settle_epoch(&mut vault, 50, 0);
    let state = vault.state().unwrap();
    let snapshot = state
        .epoch
        .last_settled
        .as_ref()
        .expect("accepted settlement snapshot");
    let claim = snapshot
        .claim_for(escrowed)
        .expect("derived settled claim");
    assert!(claim > 0);
    let (head_id, head) = state.withdraw_queue.head().expect("settled queue head");
    assert_eq!(head.escrow_shares, escrowed);

    let op_id = 7_007u64;
    let escrow_address = test_config().vault_address;
    let started = start_withdrawal(
        OpState::Idle,
        WithdrawalRequest {
            op_id,
            request_id: head_id,
            amount: claim,
            receiver: head.receiver,
            owner: head.owner,
            escrow_shares: escrowed,
        },
    )
    .expect("collection of the queued head starts");

    // A partial collection can never authorize a payout.
    let partial = withdrawal_step_callback(started.new_state.clone(), op_id, claim - 1)
        .expect("partial collection recorded")
        .new_state;
    assert!(withdrawal_settled(partial.clone(), state, op_id, MIN_WITHDRAWAL_ASSETS).is_err());
    assert!(payout_complete(partial, state, true, op_id, escrow_address, MIN_WITHDRAWAL_ASSETS)
        .is_err());

    // Collecting exactly the derived settled claim authorizes payout, and
    // the full recorded escrow is the only permitted burn.
    let collected = withdrawal_step_callback(started.new_state, op_id, claim)
        .expect("full collection recorded")
        .new_state;
    let authorized =
        withdrawal_settled(collected, state, op_id, MIN_WITHDRAWAL_ASSETS)
            .expect("settled claim authorizes payout");
    let OpState::Payout(payout) = &authorized.new_state else {
        panic!("lawful settlement reaches payout state");
    };
    assert_eq!(payout.amount, claim);
    assert_eq!(payout.burn_shares, escrowed);
    assert_eq!(payout.escrow_shares, escrowed);

    // A failed transfer refunds all escrowed shares, burns none, pays
    // nothing, and returns the vault to idle.
    let failed = payout_complete(
        authorized.new_state.clone(),
        state,
        false,
        op_id,
        escrow_address,
        MIN_WITHDRAWAL_ASSETS,
    )
    .expect("failed payout settles");
    assert!(matches!(failed.new_state, OpState::Idle));
    assert_eq!(
        failed
            .effects
            .iter()
            .filter(|effect| {
                matches!(effect, KernelEffect::TransferShares { from, to, shares }
                    if *from == escrow_address && *to == head.owner && *shares == escrowed)
            })
            .count(),
        1
    );
    assert!(!failed
        .effects
        .iter()
        .any(|effect| matches!(effect, KernelEffect::BurnShares { .. })));
    assert!(!failed
        .effects
        .iter()
        .any(|effect| matches!(effect, KernelEffect::TransferAssets { .. })));
    assert!(failed.effects.iter().any(|effect| {
        matches!(effect, KernelEffect::EmitEvent {
            event: KernelEvent::PayoutCompleted {
                success: false,
                burn_shares: 0,
                refund_shares,
                amount: 0,
                ..
            },
        } if *refund_shares == escrowed)
    }));

    // The pure-law settlement probe left no burn, no escrow outflow, and
    // no accounting change: the head is still queued and unreconciled.
    let after = vault.state().unwrap();
    assert!(after.withdraw_queue.head().is_some());
    assert_eq!(after.total_shares, 10_000);
    assert_eq!(after.total_assets, 10_000);
    assert_eq!(after.idle_assets, 10_000);
    assert_eq!(after.external_assets, 0);
    assert!(after.op_state.is_idle());
    assert!(after.check_invariant());
    assert!(!vault.interpreter.effects.iter().any(|effect| {
        matches!(effect, KernelEffect::BurnShares { .. })
            || matches!(effect, KernelEffect::TransferShares { from, .. } if *from == escrow_address)
    }));
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
        assert_eq!(stored_state.fee_anchor.total_assets, 800);
        assert_eq!(
            stored_state.fee_anchor.timestamp_ns,
            templar_vault_kernel::TimestampNs(100_000_000_000)
        );
    });
}

#[rstest]
fn soroban_contract_resync_idle_balance_anchors_fee_refresh_window() {
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

    env.as_contract(&contract_id, || {
        proxy
            .execute_governance_unit(
                &governance,
                &GovernanceCommand::SetGovernancePolicy {
                    kind: GOVERNANCE_POLICY_KIND_FEES,
                    target_ids: None,
                    mode: None,
                    accounts: Some(vec![
                        sdk_wire(&performance_recipient),
                        sdk_wire(&management_recipient),
                    ]),
                    market_id: None,
                    cap_group_id: None,
                    value: Some(0),
                    value_b: Some(management_fee_wad.as_u128_trunc() as i128),
                    value_c: None,
                },
            )
            .unwrap();
        proxy
            .execute_unit(&VaultCommand::ResyncIdleBalance)
            .unwrap();

        let stored_state = SorobanStorage::new(&env)
            .load_state()
            .expect("load state")
            .expect("state present");
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
        STARTING_ASSETS,
        management_fee_wad,
        RESYNC_NS,
        REFRESH_NS,
    )
    .as_u128_saturating();

    env.as_contract(&contract_id, || {
        proxy.execute_unit(&VaultCommand::RefreshFees).unwrap();

        let stored_state = SorobanStorage::new(&env)
            .load_state()
            .expect("load state")
            .expect("state present");
        assert_eq!(
            stored_state.total_shares,
            STARTING_ASSETS + expected_management_shares
        );
        assert_eq!(
            share_client.balance(&management_recipient),
            expected_management_shares as i128
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
