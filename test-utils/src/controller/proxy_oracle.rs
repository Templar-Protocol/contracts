use std::collections::HashMap;

use near_sdk::{
    json_types::Base64VecU8,
    serde::{de::DeserializeOwned, Serialize},
    serde_json::json,
    AccountId,
};
use near_workspaces::{Account, Contract};
use templar_common::{
    governance,
    oracle::pyth::{OracleResponse, PriceIdentifier},
    Nanoseconds,
};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerSet, CircuitBreakerSetConfig},
    Proxy,
};
use templar_proxy_oracle_near_common::{
    cache::{CachedProxyPrice, CachedProxyPriceStatus},
    governance::{CircuitBreakerUpdate, Operation},
    input::Source,
    role::Role,
    state,
};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::{migration::MigrationController, ContractController};

pub struct ProxyOracleController {
    pub contract: Contract,
}

impl ContractController for ProxyOracleController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl MigrationController for ProxyOracleController {
    type Migration = state::migration::Migration;
}

impl ProxyOracleController {
    pub const fn wasm_v0() -> &'static [u8] {
        include_bytes!("wasm/proxy_oracle_v0.wasm")
    }

    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| {
            get_contract(
                "templar_proxy_oracle_near_contract",
                "contract/proxy-oracle/near/contract",
            )
        })
        .await
    }

    pub async fn deploy(account: Account) -> Self {
        let contract = account
            .deploy(Self::wasm().await)
            .await
            .expect("proxy oracle deploy RPC failed")
            .into_result()
            .expect("proxy oracle deploy transaction failed");
        contract
            .call("new")
            .args_json(json!({}))
            .transact()
            .await
            .expect("proxy oracle init RPC failed")
            .into_result()
            .expect("proxy oracle init transaction failed");

        Self { contract }
    }

    pub async fn set_proxy(
        &self,
        executor: &Account,
        id: PriceIdentifier,
        proxy: Option<Proxy<Source>>,
    ) {
        let op_id = self.gov_next_id().await;
        self.gov_create(executor, op_id, Operation::SetProxy { id, proxy })
            .await;
        self.gov_execute(executor, op_id).await;
    }

    pub async fn add_circuit_breaker(
        &self,
        executor: &Account,
        id: PriceIdentifier,
        breaker_id: u32,
        breaker: CircuitBreaker,
    ) {
        let op_id = self.gov_next_id().await;
        self.gov_create(
            executor,
            op_id,
            Operation::AddCircuitBreaker {
                id,
                breaker_id,
                breaker,
            },
        )
        .await;
        self.gov_execute(executor, op_id).await;
    }

    pub async fn set_circuit_breaker_set_config(
        &self,
        executor: &Account,
        id: PriceIdentifier,
        config: CircuitBreakerSetConfig,
    ) {
        let op_id = self.gov_next_id().await;
        self.gov_create(
            executor,
            op_id,
            Operation::ConfigureCircuitBreakers { id, config },
        )
        .await;
        self.gov_execute(executor, op_id).await;
    }

    pub async fn set_circuit_breaker_set_manual_trip(
        &self,
        executor: &Account,
        id: PriceIdentifier,
        is_manually_tripped: bool,
    ) {
        let op_id = self.gov_next_id().await;
        self.gov_create(
            executor,
            op_id,
            Operation::SetCircuitBreakerManualTrip {
                id,
                is_manually_tripped,
            },
        )
        .await;
        self.gov_execute(executor, op_id).await;
    }

    pub async fn set_circuit_breaker_role(
        &self,
        executor: &Account,
        account_id: AccountId,
        role: Role,
        is_granted: bool,
    ) {
        let op_id = self.gov_next_id().await;
        self.gov_create(
            executor,
            op_id,
            Operation::SetCircuitBreakerRole {
                account_id,
                role,
                is_granted,
            },
        )
        .await;
        self.gov_execute(executor, op_id).await;
    }

    pub async fn remove_circuit_breaker(
        &self,
        executor: &Account,
        id: PriceIdentifier,
        breaker_id: u32,
    ) {
        let op_id = self.gov_next_id().await;
        self.gov_create(
            executor,
            op_id,
            Operation::RemoveCircuitBreaker { id, breaker_id },
        )
        .await;
        self.gov_execute(executor, op_id).await;
    }

    pub async fn update_circuit_breaker(
        &self,
        executor: &Account,
        id: PriceIdentifier,
        breaker_id: u32,
        update: CircuitBreakerUpdate,
    ) {
        let op_id = self.gov_next_id().await;
        self.gov_create(
            executor,
            op_id,
            Operation::UpdateCircuitBreaker {
                id,
                breaker_id,
                update,
            },
        )
        .await;
        self.gov_execute(executor, op_id).await;
    }

    define! {
        #[view] pub fn list_proxies(offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier>;
        #[view] pub fn get_proxy(id: PriceIdentifier) -> Option<Proxy<Source>>;
        #[view] pub fn get_proxy_circuit_breaker_set(id: PriceIdentifier) -> Option<CircuitBreakerSet>;
        #[view] pub fn get_cached_proxy_price(id: PriceIdentifier) -> Option<CachedProxyPrice>;
        #[view] pub fn list_cached_proxy_prices(price_ids: Vec<PriceIdentifier>) -> HashMap<PriceIdentifier, Option<CachedProxyPrice>>;
        #[view] pub fn has_role(account_id: AccountId, role: Role) -> bool;
        #[view] pub fn list_role(role: Role, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId>;

        #[call]
        pub fn price_feed_exists(price_identifier: PriceIdentifier) -> bool;
        #[call]
        pub fn set_circuit_breaker_manual_trip(id: PriceIdentifier, is_manually_tripped: bool, metadata: Option<Base64VecU8>);
        #[call(exec)]
        pub fn set_circuit_breaker_manual_trip_exec["set_circuit_breaker_manual_trip"](id: PriceIdentifier, is_manually_tripped: bool, metadata: Option<Base64VecU8>);
        #[call(exec)]
        pub fn price_feed_exists_exec["price_feed_exists"](price_identifier: PriceIdentifier) -> bool;
        #[call(tgas(15))]
        pub fn list_ema_prices_no_older_than(price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;
        #[call(exec, tgas(15))]
        pub fn list_ema_prices_no_older_than_exec["list_ema_prices_no_older_than"](price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;
        #[call(tgas(25))]
        pub fn update_prices(price_ids: Vec<PriceIdentifier>) -> HashMap<PriceIdentifier, CachedProxyPriceStatus>;
        #[call(exec, tgas(25))]
        pub fn update_prices_exec["update_prices"](price_ids: Vec<PriceIdentifier>) -> HashMap<PriceIdentifier, CachedProxyPriceStatus>;
    }
}

impl GovernanceController<Operation> for ProxyOracleController {}

pub trait GovernanceController<T: DeserializeOwned + Serialize>: ContractController {
    define! {
        #[view] fn gov_next_id() -> u32;
        #[view] fn gov_ttl_ns() -> Nanoseconds;
        #[view] fn gov_count() -> u32;
        #[view] fn gov_list(offset: Option<u32>, count: Option<u32>) -> Vec<u32>;
        #[view] fn gov_get(id: u32) -> Option<governance::Proposal<T>>;

        #[call(yocto(1))]
        fn gov_create(id: u32, operation: T) -> governance::Proposal<T>;
        #[call(exec, yocto(1))]
        fn gov_cancel(id: u32);
        #[call(exec, yocto(1))]
        fn gov_execute(id: u32);
    }
}
