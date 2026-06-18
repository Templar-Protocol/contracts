use near_sdk::serde_json::json;
use near_workspaces::{Account, Contract};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

#[derive(Clone)]
pub struct ReceiverController {
    pub contract: Contract,
}

impl ContractController for ReceiverController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl ReceiverController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| get_contract("mock_receiver", "mock/receiver"))
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
        pub fn get_ft_calls() -> u64;

        #[view]
        pub fn get_mt_calls() -> u64;
    }
}
