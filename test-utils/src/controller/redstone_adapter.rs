use std::collections::HashMap;

use near_sdk::{
    json_types::{Base64VecU8, U64},
    serde_json::json,
    AccountId,
};
use near_workspaces::{Account, Contract};
use templar_common::oracle::{
    redstone::{config::Config, FeedData, FeedId, GetPrices, Role, SerializableU256},
    time::Milliseconds,
};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

pub struct RedStoneAdapterController {
    pub contract: Contract,
}

impl ContractController for RedStoneAdapterController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl RedStoneAdapterController {
    pub async fn deploy(account: Account, config: Config) -> Self {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM
            .get_or_init(|| {
                get_contract(
                    "templar_redstone_adapter_contract",
                    "contract/redstone-adapter",
                )
            })
            .await;

        let contract = account.deploy(wasm).await.unwrap().unwrap();
        contract
            .call("new")
            .args_json(json!({
                "config": config,
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        Self { contract }
    }

    define! {
        #[view] pub fn has_role(account_id: AccountId, role: Role) -> bool;
        #[view] pub fn list_role(role: Role) -> Vec<AccountId>;

        #[call(exec, yocto(1))]
        pub fn set_role(account_id: AccountId, role: Role, set: Option<bool>);
    }
}

impl RedStoneAdapterInterface for RedStoneAdapterController {}

pub trait RedStoneAdapterInterface: ContractController {
    define! {
        #[view] fn get_config() -> Config;
        #[view] fn unique_signer_threshold() -> U64;
        #[view] fn get_prices(feed_ids: Vec<FeedId>, payload: Base64VecU8) -> GetPrices;
        #[view] fn read_prices(feed_ids: Vec<FeedId>) -> HashMap<FeedId, SerializableU256>;
        #[view] fn read_timestamp(feed_id: FeedId) -> Option<Milliseconds>;
        #[view] fn read_price_data_for_feed(feed_id: FeedId) -> Option<FeedData>;
        #[view] fn read_price_data(feed_ids: Vec<FeedId>) -> HashMap<FeedId, FeedData>;

        #[call(exec)]
        fn write_prices(feed_ids: Vec<FeedId>, payload: Base64VecU8);
    }
}
