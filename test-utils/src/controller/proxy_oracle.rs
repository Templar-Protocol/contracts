use near_sdk::{json_types::U64, serde_json::json};
use near_workspaces::{Account, Contract};
use templar_common::oracle::{
    proxy::{governance::Operation, Proxy},
    pyth::{OracleResponse, PriceIdentifier},
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
    pub async fn deploy(account: Account) -> Self {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM
            .get_or_init(|| get_contract("templar_proxy_oracle_contract", "contract/proxy-oracle"))
            .await;

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

    pub async fn set_proxy(
        &self,
        executor: &Account,
        id: PriceIdentifier,
        proxy: Option<Proxy>,
    ) -> u32 {
        self.propose(executor, Operation::SetProxy { id, proxy })
            .await
    }

    define! {
        #[view] pub fn list_proxies(offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier>;
        #[view] pub fn get_proxy(id: PriceIdentifier) -> Option<Proxy>;
        #[view] pub fn get_proposal_ttl_ms() -> U64;

        // Governance functions
        #[call(exec, yocto(1))]
        pub fn execute(op_id: u32);
        #[call(exec, yocto(1))]
        pub fn cancel(op_id: u32);
        #[call(yocto(1))]
        pub fn propose(operation: Operation) -> u32;

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
