use near_sdk::{serde_json::json, AccountId};
use near_workspaces::{Account, Contract};
use templar_common::oracle::pyth::{OracleResponse, PriceIdentifier};
use templar_proxy_oracle_near_common::price_transformer::PriceTransformer;
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

pub struct LstOracleController {
    pub contract: Contract,
}

impl ContractController for LstOracleController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl LstOracleController {
    pub async fn deploy(account: Account, oracle_id: AccountId) -> Self {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM
            .get_or_init(|| {
                get_contract(
                    "templar_lst_oracle_contract",
                    "contract/proxy-oracle/near/lst-contract",
                )
            })
            .await;

        let contract = account.deploy(wasm).await.unwrap().unwrap();
        contract
            .call("new")
            .args_json(json!({
                "oracle_id": oracle_id,
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        Self { contract }
    }

    define! {
        #[view] pub fn oracle_id() -> AccountId;
        #[view] pub fn list_transformers(offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier>;
        #[view] pub fn get_transformer(price_identifier: PriceIdentifier) -> Option<PriceTransformer>;

        #[call]
        pub fn price_feed_exists(price_identifier: PriceIdentifier) -> bool;
        #[call(exec)]
        pub fn price_feed_exists_exec["price_feed_exists"](price_identifier: PriceIdentifier) -> bool;
        #[call(tgas(15))]
        pub fn list_ema_prices_no_older_than(price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;
        #[call(exec, tgas(15))]
        pub fn list_ema_prices_no_older_than_exec["list_ema_prices_no_older_than"](price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;
        #[call(exec, yocto(1))]
        pub fn create_transformer(price_identifier: PriceIdentifier, entry: PriceTransformer);
    }
}
