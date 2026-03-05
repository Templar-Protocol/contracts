use soroban_sdk::{contract, contractimpl, Env};
use templar_soroban_runtime::{
    contract::{AllocationDelta, ContractConfig, CuratorVault, Delta},
    storage::{SorobanStorage, VersionedState},
    Storage,
};
use templar_vault_kernel::state::queue::DEFAULT_COOLDOWN_NS;

mod common;
use common::{MockInterpreter, TestPermissiveAuth};

type SorobanTestVault<'a> = CuratorVault<SorobanStorage<'a>, TestPermissiveAuth, MockInterpreter>;

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

fn user_addr() -> [u8; 32] {
    [10u8; 32]
}

fn allocator_addr() -> [u8; 32] {
    [3u8; 32]
}

fn fresh_loaded_vault<'a>(env: &'a Env) -> SorobanTestVault<'a> {
    let mut vault = CuratorVault::new(
        test_config(),
        SorobanStorage::new(env),
        TestPermissiveAuth,
        MockInterpreter::new(),
    );
    vault.load_state().unwrap();
    vault
}

fn assert_accounting_invariant(vault: &SorobanTestVault<'_>) {
    let state = vault.state().unwrap();
    assert_eq!(
        state.total_assets,
        state.idle_assets + state.external_assets
    );
}

fn assert_state_roundtrip(vault: &SorobanTestVault<'_>) {
    let persisted: VersionedState = vault
        .storage
        .load_state()
        .unwrap()
        .expect("state must be persisted");
    assert_eq!(
        persisted.state.total_assets,
        persisted.state.idle_assets + persisted.state.external_assets
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

        vault
            .execute_withdraw(user, 400 + DEFAULT_COOLDOWN_NS + 1)
            .unwrap();
        drop(vault);

        let vault = fresh_loaded_vault(&env);
        assert_state_roundtrip(&vault);
        assert_accounting_invariant(&vault);
        assert!(vault.state().unwrap().withdraw_queue.is_empty());
        assert_eq!(vault.state().unwrap().idle_assets, 4_000);
        assert_eq!(vault.state().unwrap().external_assets, 8_000);
        assert_eq!(vault.state().unwrap().total_assets, 12_000);
    });
}
