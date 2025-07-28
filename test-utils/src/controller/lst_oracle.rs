use near_sdk::{serde_json::json, AccountId};
use near_workspaces::{Account, Contract};
use templar_common::oracle::{
    price_transformer::PriceTransformer,
    pyth::{self, OracleResponse, PriceIdentifier},
};
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
    pub async fn deploy(account: Account, oracle_id: &AccountId) -> Self {
        static WASM_MOCK_ORACLE: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM_MOCK_ORACLE
            .get_or_init(|| get_contract("templar_lst_oracle_contract", "contract/lst-oracle"))
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
        #[view] pub fn get_oracle_id() -> AccountId;
        #[view] pub fn list_transformers(offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier>;
        #[view] pub fn get_transformer(price_identifier: PriceIdentifier) -> Option<PriceTransformer>;

        #[call]
        pub fn list_ema_prices_no_older_than(price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;
        #[call]
        pub fn set_price(price_identifier: PriceIdentifier, price: pyth::Price);
        #[call]
        pub fn create_transformer(entry: PriceTransformer) -> PriceIdentifier;
    }
}
