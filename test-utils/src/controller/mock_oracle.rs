use near_sdk::serde_json::json;
use near_workspaces::{Account, Contract};
use templar_common::oracle::{
    pyth::{self, OracleResponse, PriceIdentifier},
    redstone::{FeedData, FeedId},
};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::{redstone_adapter::RedStoneAdapterInterface, ContractController};

#[derive(Clone)]
pub struct MockOracleController {
    pub contract: Contract,
}

impl ContractController for MockOracleController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl RedStoneAdapterInterface for MockOracleController {}

impl MockOracleController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| get_contract("mock_oracle", "mock/oracle"))
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

    define! {
        #[view]
        pub fn list_ema_prices_no_older_than(price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;

        #[view]
        pub fn list_ema_prices_unsafe(price_ids: Vec<PriceIdentifier>) -> OracleResponse;

        #[view]
        pub fn last_pyth_update_data() -> Option<String>;

        #[view]
        pub fn pyth_update_count() -> near_sdk::json_types::U64;

        #[call(exec)]
        pub fn set_pyth_price(price_identifier: PriceIdentifier, price: Option<pyth::Price>);

        #[call(exec)]
        pub fn update_price_feeds(data: String);

        #[call(exec)]
        pub fn set_redstone_price(feed_id: FeedId, data: Option<FeedData>);
    }
}
