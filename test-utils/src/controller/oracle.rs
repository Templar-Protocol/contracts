use std::sync::Arc;

use near_sdk::serde_json::json;
use templar_common::oracle::pyth::{self, OracleResponse, PriceIdentifier};
use tokio::sync::OnceCell;

use crate::{define, get_contract, TestAccount};

use super::ContractController;

#[derive(Clone)]
pub struct OracleController {
    pub account: TestAccount,
}

impl ContractController for OracleController {
    fn account(&self) -> &TestAccount {
        &self.account
    }
}

impl OracleController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();
        WASM.get_or_init(|| get_contract("mock_oracle", "mock/oracle"))
            .await
    }

    pub async fn deploy(account: TestAccount) -> Self {
        near_api::Contract::deploy(account.id.clone())
            .use_code(Self::wasm().await.to_vec())
            .with_init_call("new", json!({}))
            .unwrap()
            .with_signer(Arc::clone(&account.signer))
            .send_to(&account.network)
            .await
            .unwrap()
            .assert_success();

        Self { account }
    }

    define! {
        #[view]
        pub fn list_ema_prices_no_older_than(price_ids: Vec<PriceIdentifier>, age: u32) -> OracleResponse;

        #[call(exec)]
        pub fn set_price(price_identifier: PriceIdentifier, price: pyth::Price);
    }
}
