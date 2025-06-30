use near_sdk::{json_types::U128, serde_json::json, AccountId};
use near_workspaces::{Account, Contract};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

pub struct MtController {
    pub contract: Contract,
}

impl ContractController for MtController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl MtController {
    pub async fn deploy(account: Account) -> Self {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM
            .get_or_init(|| get_contract("mock_mt", "mock/mt"))
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
        pub fn mt_balance_of(token_id: String, account_id: &AccountId) -> U128;

        #[call(yocto(1))]
        pub fn mt_transfer(token_id: String, receiver_id: &AccountId, amount: U128);

        #[call(yocto(1), tgas(300))]
        pub fn mt_transfer_call(token_id: String, receiver_id: &AccountId, amount: U128, msg: String);

        #[call]
        pub fn mint(token_id: String, amount: U128);
    }
}
