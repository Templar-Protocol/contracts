#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value.
#![allow(clippy::needless_pass_by_value)]

use soroban_sdk::{contract, contracterror, contractimpl, Address, Env, Symbol, Vec};
use templar_proxy_oracle_soroban_common::{
    Asset, ProxyOracleMaintenanceClient, RefreshStatus, DEFAULT_TTL_EXTEND_TO,
    DEFAULT_TTL_THRESHOLD,
};

pub const MAX_BATCH_ITEMS: u32 = 64;

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BatcherError {
    TooManyItems = 1,
}

/// Stateless fan-out so a keeper can service every asset in one Soroban
/// operation. Holds no authority: every call it forwards is permissionless on
/// the target. An archived instance or code entry, or a non-contract address,
/// fails the whole operation at the ledger level (nothing here can catch it);
/// restore such targets before batching them.
#[contract]
pub struct ProxyOracleBatcher;

#[contractimpl]
impl ProxyOracleBatcher {
    /// `refresh(asset)` on `oracle` for each asset, in order. A trap in any
    /// refresh reverts the whole batch; status-level failures are returned.
    pub fn refresh_many(env: Env, oracle: Address, assets: Vec<Asset>) -> Vec<RefreshStatus> {
        require_batch_len(&env, &assets);
        extend_self(&env);
        let client = ProxyOracleMaintenanceClient::new(&env, &oracle);
        let mut statuses = Vec::new(&env);
        for asset in assets.iter() {
            statuses.push_back(client.refresh(&asset));
        }
        statuses
    }

    /// Renew `oracle`'s instance and code, then `extend_ttl(asset)` on it for
    /// each asset; `false` marks an asset the runtime rejected (unregistered).
    pub fn extend_ttl_many(env: Env, oracle: Address, assets: Vec<Asset>) -> Vec<bool> {
        require_batch_len(&env, &assets);
        extend_self(&env);
        extend_instance_and_code(&env, oracle.clone());
        let client = ProxyOracleMaintenanceClient::new(&env, &oracle);
        let mut results = Vec::new(&env);
        for asset in assets.iter() {
            results.push_back(matches!(client.try_extend_ttl(&asset), Ok(Ok(()))));
        }
        results
    }

    /// Renew each contract's instance and code, then call its argument-less
    /// `extend_ttl()` for its persistent entries; `false` marks a failed
    /// `extend_ttl()` invocation.
    pub fn extend_ttl_contracts(env: Env, contracts: Vec<Address>) -> Vec<bool> {
        require_batch_len(&env, &contracts);
        extend_self(&env);
        let extend_ttl = Symbol::new(&env, "extend_ttl");
        let no_args = Vec::new(&env);
        let mut results = Vec::new(&env);
        for contract in contracts.iter() {
            extend_instance_and_code(&env, contract.clone());
            let outcome = env.try_invoke_contract::<(), soroban_sdk::Error>(
                &contract,
                &extend_ttl,
                no_args.clone(),
            );
            results.push_back(matches!(outcome, Ok(Ok(()))));
        }
        results
    }
}

fn require_batch_len<T>(env: &Env, items: &Vec<T>) {
    if items.len() > MAX_BATCH_ITEMS {
        env.panic_with_error(BatcherError::TooManyItems);
    }
}

fn extend_self(env: &Env) {
    extend_instance_and_code(env, env.current_contract_address());
}

fn extend_instance_and_code(env: &Env, contract: Address) {
    env.deployer()
        .extend_ttl(contract, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}

#[cfg(test)]
mod tests {
    use soroban_sdk::{
        contract, contractimpl, symbol_short,
        testutils::{
            storage::{Instance as _, Persistent as _},
            Address as _, Deployer as _,
        },
        Address, Env, Error, Symbol, Vec,
    };
    use templar_proxy_oracle_soroban_common::{ContractError, ProxyOracleMaintenanceTrait};

    use super::{
        Asset, BatcherError, ProxyOracleBatcher, ProxyOracleBatcherClient, RefreshStatus,
        MAX_BATCH_ITEMS,
    };

    const CALLS: Symbol = symbol_short!("CALLS");
    const DATA: Symbol = symbol_short!("DATA");

    #[contract]
    struct MockMaintenance;

    #[contractimpl]
    impl MockMaintenance {
        pub fn calls(env: Env) -> u32 {
            env.storage().instance().get(&CALLS).unwrap_or(0)
        }

        pub fn asset_calls(env: Env, asset: Asset) -> u32 {
            env.storage().instance().get(&asset).unwrap_or(0)
        }
    }

    #[contractimpl]
    impl ProxyOracleMaintenanceTrait for MockMaintenance {
        fn refresh(env: Env, asset: Asset) -> RefreshStatus {
            record_call(&env);
            record_asset_call(&env, &asset);
            if asset == Asset::Other(Symbol::new(&env, "TRAP")) {
                env.panic_with_error(BatcherError::TooManyItems);
            }
            if asset == Asset::Other(Symbol::new(&env, "UNKNOWN")) {
                RefreshStatus::UnknownAsset
            } else {
                RefreshStatus::Blocked(1)
            }
        }

        fn extend_ttl(env: Env, asset: Asset) -> Result<(), ContractError> {
            record_call(&env);
            record_asset_call(&env, &asset);
            if asset == Asset::Other(Symbol::new(&env, "TRAP")) {
                env.panic_with_error(BatcherError::TooManyItems);
            }
            Ok(())
        }
    }

    #[contract]
    struct MockTtl;

    #[contractimpl]
    impl MockTtl {
        pub fn extend_ttl(env: Env) {
            record_call(&env);
        }

        pub fn calls(env: Env) -> u32 {
            env.storage().instance().get(&CALLS).unwrap_or(0)
        }
    }

    #[contract]
    struct MockFailingTtl;

    #[contractimpl]
    impl MockFailingTtl {
        pub fn __constructor(env: Env) {
            env.storage().persistent().set(&DATA, &1_u32);
        }

        pub fn extend_ttl(env: Env) {
            record_call(&env);
            env.storage().persistent().set(&DATA, &2_u32);
            env.storage()
                .persistent()
                .extend_ttl(&DATA, 518_400, 3_000_000);
            env.panic_with_error(BatcherError::TooManyItems);
        }

        pub fn calls(env: Env) -> u32 {
            env.storage().instance().get(&CALLS).unwrap_or(0)
        }

        pub fn value(env: Env) -> u32 {
            env.storage().persistent().get(&DATA).unwrap_or(0)
        }
    }

    fn record_call(env: &Env) {
        let calls = env.storage().instance().get::<_, u32>(&CALLS).unwrap_or(0);
        env.storage().instance().set(&CALLS, &(calls + 1));
    }

    fn record_asset_call(env: &Env, asset: &Asset) {
        let calls = env.storage().instance().get::<_, u32>(asset).unwrap_or(0);
        env.storage().instance().set(asset, &(calls + 1));
    }

    fn instance_ttl(env: &Env, contract: &Address) -> u32 {
        env.as_contract(contract, || env.storage().instance().get_ttl())
    }

    fn persistent_ttl(env: &Env, contract: &Address) -> u32 {
        env.as_contract(contract, || env.storage().persistent().get_ttl(&DATA))
    }

    fn code_ttl(env: &Env, contract: &Address) -> u32 {
        env.deployer().get_contract_code_ttl(contract)
    }

    #[test]
    fn every_entrypoint_rejects_65_items_before_dispatch() {
        let env = Env::default();
        let batcher_id = env.register(ProxyOracleBatcher, ());
        let batcher = ProxyOracleBatcherClient::new(&env, &batcher_id);
        let target = Address::generate(&env);
        let mut assets = Vec::new(&env);
        let mut contracts = Vec::new(&env);
        for _ in 0..=MAX_BATCH_ITEMS {
            assets.push_back(Asset::Other(Symbol::new(&env, "BTC")));
            contracts.push_back(target.clone());
        }

        assert_eq!(
            batcher.try_refresh_many(&target, &assets),
            Err(Ok(Error::from_contract_error(
                BatcherError::TooManyItems as u32
            )))
        );
        assert_eq!(
            batcher.try_extend_ttl_many(&target, &assets),
            Err(Ok(Error::from_contract_error(
                BatcherError::TooManyItems as u32
            )))
        );
        assert_eq!(
            batcher.try_extend_ttl_contracts(&contracts),
            Err(Ok(Error::from_contract_error(
                BatcherError::TooManyItems as u32
            )))
        );
    }

    #[test]
    fn every_entrypoint_accepts_64_items_without_authorization() {
        let env = Env::default();
        let batcher_id = env.register(ProxyOracleBatcher, ());
        let batcher = ProxyOracleBatcherClient::new(&env, &batcher_id);
        let maintenance_id = env.register(MockMaintenance, ());
        let maintenance = MockMaintenanceClient::new(&env, &maintenance_id);
        let ttl_id = env.register(MockTtl, ());
        let ttl = MockTtlClient::new(&env, &ttl_id);
        let mut assets = Vec::new(&env);
        let mut contracts = Vec::new(&env);
        for _ in 0..MAX_BATCH_ITEMS {
            assets.push_back(Asset::Other(Symbol::new(&env, "BTC")));
            contracts.push_back(ttl_id.clone());
        }
        env.mock_auths(&[]);

        assert_eq!(
            batcher.refresh_many(&maintenance_id, &assets).len(),
            MAX_BATCH_ITEMS
        );
        assert_eq!(
            batcher.extend_ttl_many(&maintenance_id, &assets).len(),
            MAX_BATCH_ITEMS
        );
        assert_eq!(maintenance.calls(), MAX_BATCH_ITEMS * 2);
        assert_eq!(
            batcher.extend_ttl_contracts(&contracts).len(),
            MAX_BATCH_ITEMS
        );
        assert_eq!(ttl.calls(), MAX_BATCH_ITEMS);
    }

    #[test]
    fn empty_batches_succeed_without_dispatch() {
        let env = Env::default();
        let batcher_id = env.register(ProxyOracleBatcher, ());
        let batcher = ProxyOracleBatcherClient::new(&env, &batcher_id);
        let maintenance_id = env.register(MockMaintenance, ());
        let maintenance = MockMaintenanceClient::new(&env, &maintenance_id);
        env.mock_auths(&[]);

        assert!(batcher
            .refresh_many(&maintenance_id, &Vec::new(&env))
            .is_empty());
        assert!(batcher
            .extend_ttl_many(&maintenance_id, &Vec::new(&env))
            .is_empty());
        assert!(batcher
            .extend_ttl_contracts(&Vec::<Address>::new(&env))
            .is_empty());
        assert_eq!(maintenance.calls(), 0);
    }

    #[test]
    fn refresh_preserves_positions_and_invokes_duplicate_assets() {
        let env = Env::default();
        let batcher_id = env.register(ProxyOracleBatcher, ());
        let batcher = ProxyOracleBatcherClient::new(&env, &batcher_id);
        let maintenance_id = env.register(MockMaintenance, ());
        let maintenance = MockMaintenanceClient::new(&env, &maintenance_id);
        let known = Asset::Other(Symbol::new(&env, "BTC"));
        let unknown = Asset::Other(Symbol::new(&env, "UNKNOWN"));
        let assets = Vec::from_array(&env, [known.clone(), unknown.clone(), known.clone()]);
        env.mock_auths(&[]);

        assert_eq!(
            batcher.refresh_many(&maintenance_id, &assets),
            Vec::from_array(
                &env,
                [
                    RefreshStatus::Blocked(1),
                    RefreshStatus::UnknownAsset,
                    RefreshStatus::Blocked(1),
                ],
            )
        );
        assert_eq!(maintenance.asset_calls(&known), 2);
        assert_eq!(maintenance.asset_calls(&unknown), 1);
    }

    #[test]
    fn target_trap_rolls_back_prior_refresh_and_batcher_ttl() {
        let env = Env::default();
        let batcher_id = env.register(ProxyOracleBatcher, ());
        let batcher = ProxyOracleBatcherClient::new(&env, &batcher_id);
        let maintenance_id = env.register(MockMaintenance, ());
        let maintenance = MockMaintenanceClient::new(&env, &maintenance_id);
        let assets = Vec::from_array(
            &env,
            [
                Asset::Other(Symbol::new(&env, "BTC")),
                Asset::Other(Symbol::new(&env, "TRAP")),
            ],
        );
        let batcher_ttl_before = instance_ttl(&env, &batcher_id);

        assert!(batcher.try_refresh_many(&maintenance_id, &assets).is_err());

        assert_eq!(maintenance.calls(), 0);
        assert_eq!(instance_ttl(&env, &batcher_id), batcher_ttl_before);
    }

    #[test]
    fn invalid_ttl_target_rolls_back_prior_dispatch_and_ttl_extensions() {
        let env = Env::default();
        let batcher_id = env.register(ProxyOracleBatcher, ());
        let batcher = ProxyOracleBatcherClient::new(&env, &batcher_id);
        let ttl_id = env.register(MockTtl, ());
        let ttl = MockTtlClient::new(&env, &ttl_id);
        let invalid_target = Address::generate(&env);
        let contracts = Vec::from_array(&env, [ttl_id.clone(), invalid_target]);
        let batcher_ttl_before = instance_ttl(&env, &batcher_id);
        let target_ttl_before = instance_ttl(&env, &ttl_id);

        assert!(batcher.try_extend_ttl_contracts(&contracts).is_err());

        assert_eq!(ttl.calls(), 0);
        assert_eq!(instance_ttl(&env, &batcher_id), batcher_ttl_before);
        assert_eq!(instance_ttl(&env, &ttl_id), target_ttl_before);
    }

    #[test]
    fn caught_ttl_failure_preserves_outer_renewal_and_rolls_back_nested_effects() {
        let env = Env::default();
        let batcher_id = env.register(ProxyOracleBatcher, ());
        let batcher = ProxyOracleBatcherClient::new(&env, &batcher_id);
        let target_id = env.register(MockFailingTtl, ());
        let target = MockFailingTtlClient::new(&env, &target_id);
        let batcher_ttl_before = instance_ttl(&env, &batcher_id);
        let target_instance_before = instance_ttl(&env, &target_id);
        let target_code_before = code_ttl(&env, &target_id);
        let persistent_before = persistent_ttl(&env, &target_id);
        env.mock_auths(&[]);

        assert_eq!(
            batcher.extend_ttl_contracts(&Vec::from_array(&env, [target_id.clone()],)),
            Vec::from_array(&env, [false])
        );

        assert!(instance_ttl(&env, &batcher_id) > batcher_ttl_before);
        assert!(instance_ttl(&env, &target_id) > target_instance_before);
        assert!(code_ttl(&env, &target_id) > target_code_before);
        assert_eq!(persistent_ttl(&env, &target_id), persistent_before);
        assert_eq!(target.calls(), 0);
        assert_eq!(target.value(), 1);
    }

    #[test]
    fn ttl_entrypoints_extend_batcher_and_target_instances() {
        let env = Env::default();
        let batcher_id = env.register(ProxyOracleBatcher, ());
        let batcher = ProxyOracleBatcherClient::new(&env, &batcher_id);
        let maintenance_id = env.register(MockMaintenance, ());
        let ttl_id = env.register(MockTtl, ());
        let batcher_ttl_before = instance_ttl(&env, &batcher_id);
        let maintenance_ttl_before = instance_ttl(&env, &maintenance_id);
        let ttl_before = instance_ttl(&env, &ttl_id);

        assert_eq!(
            batcher.extend_ttl_many(
                &maintenance_id,
                &Vec::from_array(&env, [Asset::Other(Symbol::new(&env, "BTC"))]),
            ),
            Vec::from_array(&env, [true])
        );
        assert!(instance_ttl(&env, &batcher_id) > batcher_ttl_before);
        assert!(instance_ttl(&env, &maintenance_id) > maintenance_ttl_before);

        assert_eq!(
            batcher.extend_ttl_contracts(&Vec::from_array(&env, [ttl_id.clone()])),
            Vec::from_array(&env, [true])
        );
        assert!(instance_ttl(&env, &ttl_id) > ttl_before);
    }
}
