use near_sdk::{
    serde::{de::DeserializeOwned, Serialize},
    serde_json::json,
};
use near_workspaces::{Account, Contract};
use templar_common::{
    governance,
    oracle::{
        proxy::{self, governance::Operation, Proxy},
        pyth::{OracleResponse, PriceIdentifier},
    },
    time::Nanoseconds,
};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

pub struct ProxyOracleController {
    pub contract: Contract,
}

impl ContractController for ProxyOracleController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl ProxyOracleController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| get_contract("templar_proxy_oracle_contract", "contract/proxy-oracle"))
            .await
    }

    pub async fn deploy(account: Account) -> Self {
        let wasm = Self::wasm().await;

        let contract = account.deploy(wasm).await.unwrap().unwrap();
        contract
            .call("new")
            .args_json(json!({}))
            .transact()
            .await
            .unwrap()
            .unwrap();

        Self { contract }
    }

    pub async fn set_proxy(&self, executor: &Account, id: PriceIdentifier, proxy: Option<Proxy>) {
        let op_id = self.gov_next_id().await;
        self.gov_create(executor, op_id, Operation::SetProxy { id, proxy })
            .await;
        self.gov_execute(executor, op_id).await;
    }

    define! {
        #[view] pub fn list_proxies(offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier>;
        #[view] pub fn get_proxy(id: PriceIdentifier) -> Option<Proxy>;

        #[call]
        pub fn price_feed_exists(price_identifier: PriceIdentifier) -> bool;
        #[call(exec)]
        pub fn price_feed_exists_exec["price_feed_exists"](price_identifier: PriceIdentifier) -> bool;
        #[call(tgas(15))]
        pub fn list_ema_prices_no_older_than(price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;
        #[call(exec, tgas(15))]
        pub fn list_ema_prices_no_older_than_exec["list_ema_prices_no_older_than"](price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;
    }
}

impl GovernanceController<proxy::governance::Operation> for ProxyOracleController {}

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
