use std::sync::Arc;

use near_sdk::{json_types::U128, serde_json::json, AccountId};
use tokio::sync::OnceCell;

use crate::{define, get_contract, TestAccount};

use super::ContractController;

#[derive(Clone)]
pub struct MtController {
    pub account: TestAccount,
}

impl ContractController for MtController {
    fn account(&self) -> &TestAccount {
        &self.account
    }
}

impl MtController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();
        eprintln!("MtController::wasm");
        let w = WASM
            .get_or_init(|| get_contract("mock_mt", "mock/mt"))
            .await;
        eprintln!("MtController::wasm[return]");
        w
    }

    pub async fn deploy(account: TestAccount) -> Self {
        eprintln!("MtController::deploy");
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
        pub fn mt_balance_of(token_id: String, account_id: &AccountId) -> U128;

        #[view]
        pub fn redemption_rate(token_id: String) -> U128;

        #[call(exec, yocto(1))]
        pub fn mt_transfer(token_id: String, receiver_id: &AccountId, amount: U128);

        #[call(exec, yocto(1), tgas(300))]
        pub fn mt_transfer_call(token_id: String, receiver_id: &AccountId, amount: U128, msg: String);

        #[call(exec)]
        pub fn mint(token_id: String, amount: U128);

        #[call(exec)]
        pub fn set_redemption_rate(token_id: String, redemption_rate: U128);
    }
}
