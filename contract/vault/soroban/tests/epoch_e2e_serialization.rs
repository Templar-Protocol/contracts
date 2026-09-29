// ENG-697 settlement-anchored law suite: drives immediate entrypoints and
// epoch settlement together; compiled only when both features are enabled.
#![cfg(all(feature = "epoch", feature = "immediate-entrypoints"))]
use soroban_sdk::{contract, contractimpl, Env};
use templar_curator_primitives::MarketConfig;
use templar_soroban_runtime::{
    contract::{
        AllocationDelta, ContractConfig, CuratorVault, Delta,
        SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS,
    },
    storage::SorobanStorage,
    Storage,
};
use templar_vault_kernel::{apply_action, Address, KernelAction, TimestampNs, ValuationReportRef};

mod common;
use common::{MockInterpreter, TestPermissiveAuth};

type SorobanTestVault<'a> = CuratorVault<SorobanStorage<'a>, TestPermissiveAuth, MockInterpreter>;

fn test_config() -> ContractConfig {
    ContractConfig::new(
        Address([1u8; 32]),
        Address([9u8; 32]),
        vec![Address([2u8; 32])],
        Address([4u8; 32]),
        Address([5u8; 32]),
    )
}

fn user_addr() -> Address {
    Address([10u8; 32])
}

fn allocator_addr() -> Address {
    Address([3u8; 32])
}

fn fresh_loaded_vault(env: &Env) -> SorobanTestVault<'_> {
    let mut vault = CuratorVault::new(
        test_config(),
        SorobanStorage::new(env),
        TestPermissiveAuth,
        MockInterpreter::new(),
    );
    vault.load_state().unwrap();
    vault
}

fn configure_market_zero(vault: &mut SorobanTestVault<'_>) {
    vault
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();
    let policy_state = vault.policy_state().clone();
    vault.storage.save_policy_state(&policy_state).unwrap();
}

fn assert_accounting_invariant(vault: &SorobanTestVault<'_>) {
    let state = vault.state().unwrap();
    assert_eq!(
        state.total_assets,
        state.idle_assets + state.external_assets
    );
}

fn assert_state_roundtrip(vault: &SorobanTestVault<'_>) {
    let persisted = vault
        .storage
        .load_state()
        .unwrap()
        .expect("state must be persisted");
    assert_eq!(
        persisted.total_assets,
        persisted.idle_assets + persisted.external_assets
    );
}

mod test_contract {
    use super::*;

    #[contract]
    pub struct TestContract;

    #[contractimpl]
    impl TestContract {
        pub fn noop(_env: Env) {}
    }
}

#[test]
fn e2e_soroban_storage_postcard_roundtrip_lifecycle() {
    let env = Env::default();
    let contract_id = env.register(test_contract::TestContract, ());

    env.as_contract(&contract_id, || {
        let user = user_addr();
        let allocator = allocator_addr();

        let mut vault = fresh_loaded_vault(&env);
        configure_market_zero(&mut vault);
        assert_accounting_invariant(&vault);

        vault.deposit(user, user, 10_000, 0, 100).unwrap();
        drop(vault);

        let mut vault = fresh_loaded_vault(&env);
        assert_state_roundtrip(&vault);
        assert_accounting_invariant(&vault);
        assert_eq!(vault.state().unwrap().total_assets, 10_000);
        assert_eq!(vault.state().unwrap().idle_assets, 10_000);
        assert_eq!(vault.state().unwrap().external_assets, 0);

        vault.deposit(user, user, 5_000, 0, 200).unwrap();
        drop(vault);

        let mut vault = fresh_loaded_vault(&env);
        assert_state_roundtrip(&vault);
        assert_accounting_invariant(&vault);
        assert_eq!(vault.state().unwrap().total_assets, 15_000);
        assert_eq!(vault.state().unwrap().idle_assets, 15_000);
        assert_eq!(vault.state().unwrap().external_assets, 0);

        vault
            .allocate(
                allocator,
                &AllocationDelta::Supply(Delta {
                    market: 0,
                    amount: 8_000,
                }),
            )
            .unwrap();
        drop(vault);

        let mut vault = fresh_loaded_vault(&env);
        assert_state_roundtrip(&vault);
        assert_accounting_invariant(&vault);
        assert_eq!(vault.state().unwrap().total_assets, 15_000);
        assert_eq!(vault.state().unwrap().idle_assets, 7_000);
        assert_eq!(vault.state().unwrap().external_assets, 8_000);

        vault.refresh_markets(allocator, vec![0], 300).unwrap();
        drop(vault);

        let mut vault = fresh_loaded_vault(&env);
        assert_state_roundtrip(&vault);
        assert_accounting_invariant(&vault);
        assert_eq!(vault.state().unwrap().external_assets, 8_000);

        let withdraw_result = vault
            .allocate(
                allocator,
                &AllocationDelta::Withdraw(Delta {
                    market: 0,
                    amount: 3_000,
                }),
            )
            .unwrap();
        assert_eq!(withdraw_result.op_id, 2);
        drop(vault);

        let mut vault = fresh_loaded_vault(&env);
        assert_state_roundtrip(&vault);
        assert_accounting_invariant(&vault);
        assert!(vault.state().unwrap().op_state.is_idle());
        assert_eq!(vault.state().unwrap().idle_assets, 10_000);
        assert_eq!(vault.state().unwrap().external_assets, 5_000);
        assert_eq!(vault.state().unwrap().next_op_id, 3);

        let request = vault.request_withdraw(user, user, 3_000, 0, 400).unwrap();
        drop(vault);

        let mut vault = fresh_loaded_vault(&env);
        assert_state_roundtrip(&vault);
        assert_accounting_invariant(&vault);
        let (head_id, _) = vault
            .state()
            .unwrap()
            .withdraw_queue
            .head()
            .expect("pending withdrawal request");
        assert_eq!(head_id, request.request_id);

        // Close intake and settle the epoch through kernel law before any
        // claim is priced. The payout below is derived exclusively from the
        // resulting immutable settlement snapshot, not from the request.
        let kernel_config = templar_vault_kernel::VaultConfig {
            fees: templar_vault_kernel::FeesSpec::zero(),
            min_withdrawal_assets: 1_000,
            withdrawal_cooldown_ns: SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS,
            max_pending_withdrawals: 20,
            paused: false,
            virtual_shares: 0,
            virtual_assets: 0,
        };
        let self_id = Address([9u8; 32]);
        let cutoff = apply_action(
            vault.state().unwrap().clone(),
            &kernel_config,
            None,
            &self_id,
            KernelAction::BeginEpochCutoff {
                cutoff_ns: TimestampNs(400),
                now_ns: TimestampNs(400),
            },
        )
        .expect("epoch cutoff accepted");
        let settled = apply_action(
            cutoff.state,
            &kernel_config,
            None,
            &self_id,
            KernelAction::SettleEpoch {
                report: ValuationReportRef {
                    report_seq: 1,
                    as_of_ns: TimestampNs(401),
                    report_hash: [7u8; 32],
                },
                new_external_assets: 5_000,
                max_report_age_ns: SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS,
                settle_now_ns: TimestampNs(402),
            },
        )
        .expect("epoch settlement accepted");
        assert!(settled.state.epoch.last_settled.is_some());
        *vault.state_mut().unwrap() = settled.state;
        vault.save_state().expect("settled epoch persists");

        vault
            .execute_withdraw(user, 400 + SOROBAN_DEFAULT_WITHDRAWAL_COOLDOWN_NS + 1)
            .unwrap();
        drop(vault);

        let vault = fresh_loaded_vault(&env);
        assert_state_roundtrip(&vault);
        assert_accounting_invariant(&vault);
        assert!(vault.state().unwrap().withdraw_queue.is_empty());
        assert_eq!(vault.state().unwrap().idle_assets, 7_000);
        assert_eq!(vault.state().unwrap().external_assets, 5_000);
        assert_eq!(vault.state().unwrap().total_assets, 12_000);
    });
}
