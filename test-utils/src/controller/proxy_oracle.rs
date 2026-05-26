use std::collections::HashMap;

use near_sdk::{
    json_types::Base64VecU8,
    serde::{de::DeserializeOwned, Serialize},
    serde_json::json,
};
use near_workspaces::{Account, Contract};
use templar_common::{
    oracle::pyth::{OracleResponse, PriceIdentifier},
    Nanoseconds,
};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerSet, CircuitBreakerSetConfig},
    Proxy,
};
use templar_proxy_oracle_near_common::{
    cache::{CachedProxyPrice, CachedProxyPriceStatus},
    input::Source,
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

    pub async fn admin_set_proxy(
        &self,
        _executor: &Account,
        id: PriceIdentifier,
        proxy: Option<Proxy<Source>>,
    ) {
        self.contract
            .call("admin_set_proxy")
            .args_json(json!({ "id": id, "proxy": proxy }))
            .transact()
            .await
            .unwrap()
            .into_result()
            .unwrap();
    }

    pub async fn admin_add_circuit_breaker(
        &self,
        _executor: &Account,
        id: PriceIdentifier,
        breaker_id: u32,
        breaker: CircuitBreaker,
    ) {
        self.contract
            .call("admin_add_circuit_breaker")
            .args_json(json!({ "id": id, "breaker_id": breaker_id, "breaker": breaker }))
            .transact()
            .await
            .unwrap()
            .into_result()
            .unwrap();
    }

    pub async fn admin_configure_circuit_breakers(
        &self,
        _executor: &Account,
        id: PriceIdentifier,
        config: CircuitBreakerSetConfig,
    ) {
        self.contract
            .call("admin_configure_circuit_breakers")
            .args_json(json!({ "id": id, "config": config }))
            .transact()
            .await
            .unwrap()
            .into_result()
            .unwrap();
    }

    pub async fn admin_remove_circuit_breaker(
        &self,
        _executor: &Account,
        id: PriceIdentifier,
        breaker_id: u32,
    ) {
        self.contract
            .call("admin_remove_circuit_breaker")
            .args_json(json!({ "id": id, "breaker_id": breaker_id }))
            .transact()
            .await
            .unwrap()
            .into_result()
            .unwrap();
    }

    pub async fn admin_upgrade(&self, executor: &Account, code: Vec<u8>, migrate_args: Vec<u8>) {
        executor
            .call(self.contract.id(), "admin_upgrade")
            .args_json(json!({
                "code": Base64VecU8(code),
                "migrate_args": Base64VecU8(migrate_args),
            }))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .into_result()
            .unwrap();
    }

    define! {
        #[view] pub fn list_proxies(offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier>;
        #[view] pub fn get_proxy(id: PriceIdentifier) -> Option<Proxy<Source>>;
        #[view] pub fn get_proxy_circuit_breaker_set(id: PriceIdentifier) -> Option<CircuitBreakerSet>;
        #[view] pub fn get_cached_proxy_price(id: PriceIdentifier) -> Option<CachedProxyPrice>;
        #[view] pub fn list_cached_proxy_prices(price_ids: Vec<PriceIdentifier>) -> HashMap<PriceIdentifier, Option<CachedProxyPrice>>;

        #[call]
        pub fn price_feed_exists(price_identifier: PriceIdentifier) -> bool;
        #[call]
        pub fn admin_set_manual_trip(id: PriceIdentifier, is_manually_tripped: bool, metadata: Option<Base64VecU8>);
        #[call(exec)]
        pub fn admin_set_manual_trip_exec["admin_set_manual_trip"](id: PriceIdentifier, is_manually_tripped: bool, metadata: Option<Base64VecU8>);
        #[call(exec)]
        pub fn admin_rearm_exec["admin_rearm"](id: PriceIdentifier, breaker_id: u32, armed_after_ns: Nanoseconds, accepted_history_source: templar_proxy_oracle_kernel::proxy::circuit_breaker::AcceptedHistorySource);
        #[call(exec)]
        pub fn admin_set_enforced_exec["admin_set_enforced"](id: PriceIdentifier, breaker_id: u32, is_enforced: bool);
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

pub trait GovernanceController<T: DeserializeOwned + Serialize>: ContractController {
    define! {
        #[view] fn next_proposal_id() -> u32;
        #[view] fn proposal_count() -> u32;
        #[view] fn list_proposals(offset: Option<u32>, count: Option<u32>) -> Vec<u32>;
        #[view] fn get_proposal(id: u32) -> Option<templar_proxy_oracle_near_governance_common::Proposal<T>>;
        #[view] fn get_effective_proposal_ttl(operation: T, requested_ttl: Nanoseconds) -> Nanoseconds;
        #[view] fn get_operation_ttl(kind: templar_proxy_oracle_near_governance_common::OperationKind) -> Nanoseconds;

        #[call(yocto(1))]
        fn create_proposal(id: u32, operation: T, requested_ttl: Nanoseconds) -> templar_proxy_oracle_near_governance_common::Proposal<T>;
        #[call(exec, yocto(1))]
        fn cancel_proposal(id: u32);
        #[call(exec, yocto(1))]
        fn execute_proposal(id: u32);
    }
}
