use near_sdk::{
    json_types::{Base64VecU8, U64},
    serde_json::json,
};
use near_workspaces::{Account, Contract};
use templar_common::oracle::redstone::{
    config::Config,
    feed_data::{FeedData, SerializableU256},
    GetPrices,
};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

// #[derive(Clone)]
// pub struct RedStoneAdapterController {
//     pub contract: Contract,
// }

// impl ContractController for RedStoneAdapterController {
//     fn contract(&self) -> &Contract {
//         &self.contract
//     }
// }

pub trait RedStoneAdapterController: ContractController {
    // pub fn new(contract: Contract) -> Self {
    //     Self { contract }
    // }

    // pub async fn deploy(account: Account, config: Config) -> Self {
    //     static WASM_MOCK_ORACLE: OnceCell<Vec<u8>> = OnceCell::const_new();

    //     let wasm = WASM_MOCK_ORACLE
    //         .get_or_init(|| {
    //             get_contract(
    //                 "templar_redstone_adapter_contract",
    //                 "contract/redstone-adapter",
    //             )
    //         })
    //         .await;

    //     let contract = account.deploy(wasm).await.unwrap().unwrap();
    //     contract
    //         .call("new")
    //         .args_json(json!({
    //             "config": config,
    //         }))
    //         .transact()
    //         .await
    //         .unwrap()
    //         .unwrap();

    //     Self { contract }
    // }

    define! {
        #[view] fn get_config() -> Config;
        #[view] fn unique_signer_threshold() -> U64;
        #[view] fn get_prices(feed_ids: Vec<String>, payload: Base64VecU8) -> GetPrices;
        #[view] fn read_prices(feed_ids: Vec<String>) -> Vec<SerializableU256>;
        #[view] fn read_timestamp(feed_id: String) -> U64;
        #[view] fn read_price_data_for_feed(feed_id: String) -> FeedData;
        #[view] fn read_price_data(feed_ids: Vec<String>) -> Vec<FeedData>;

        #[call(exec)]
        fn write_prices(feed_ids: Vec<String>, payload: Base64VecU8);
    }
}
