use std::sync::Arc;

use near_sdk::{serde_json::json, AccountId, AccountIdRef};
use tokio::sync::OnceCell;

use templar_common::oracle::{
    price_transformer::PriceTransformer,
    pyth::{OracleResponse, PriceIdentifier},
};

use super::ContractController;
use crate::{define, get_contract, TestAccount};

pub struct LstOracleController {
    pub account: TestAccount,
}

impl ContractController for LstOracleController {
    fn account(&self) -> &TestAccount {
        &self.account
    }
}

impl LstOracleController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| get_contract("templar_lst_oracle_contract", "contract/lst-oracle"))
            .await
    }

    pub async fn deploy(account: TestAccount, oracle_id: &AccountIdRef) -> Self {
        near_api::Contract::deploy(account.id.clone())
            .use_code(Self::wasm().await.to_vec())
            .with_init_call(
                "new",
                json!({
                    "oracle_id": oracle_id,
                }),
            )
            .unwrap()
            .with_signer(Arc::clone(&account.signer))
            .send_to(&account.network)
            .await
            .unwrap()
            .assert_success();

        Self { account }
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
