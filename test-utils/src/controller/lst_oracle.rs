use near_sdk::{
    serde_json::{self, json},
    AccountId, Gas, NearToken,
};
use near_workspaces::{result::ExecutionSuccess, Account, Contract};
use templar_common::oracle::{
    price_transformer::PriceTransformer,
    pyth::{OracleResponse, PriceIdentifier},
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
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM
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
        #[call(yocto(1))]
        pub fn create_transformer(price_id: PriceIdentifier, entry: PriceTransformer);
    }

    pub async fn list_ema_prices_no_older_than_exec(
        &self,
        executor: &Account,
        price_ids: impl Into<Vec<PriceIdentifier>>,
        age: impl Into<u32>,
    ) -> ExecutionSuccess {
        self.call_exec(
            executor,
            "list_ema_prices_no_older_than",
            serde_json::to_vec(&json!({
                "price_ids": price_ids.into(),
                "age": age.into(),
            }))
            .unwrap(),
            NearToken::from_near(0),
            Gas::from_tgas(20),
        )
        .await
    }
}
