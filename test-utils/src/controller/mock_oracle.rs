use near_sdk::serde_json::json;
use near_workspaces::{Account, Contract};
use templar_common::oracle::{
    pyth::{self, OracleResponse, PriceIdentifier},
    redstone::{FeedData, FeedId},
};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::{redstone_adapter::RedStoneAdapterController, ContractController};

#[derive(Clone)]
pub struct MockOracleController {
    pub contract: Contract,
}

impl ContractController for MockOracleController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl RedStoneAdapterController for MockOracleController {}

impl MockOracleController {
    pub async fn deploy(account: Account) -> Self {
        static WASM_MOCK_ORACLE: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM_MOCK_ORACLE
            .get_or_init(|| get_contract("mock_oracle", "mock/oracle"))
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

    define! {
        #[view]
        pub fn list_ema_prices_no_older_than(price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;

        #[call(exec)]
        pub fn set_pyth_price(price_identifier: PriceIdentifier, price: Option<pyth::Price>);

        #[call(exec)]
        pub fn set_redstone_price(feed_id: FeedId, data: Option<FeedData>);
    }
}
