use near_sdk::{
    json_types::U128,
    serde::{Deserialize, Serialize},
    serde_json::json,
    AccountId,
};
use near_workspaces::{Account, Contract};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

#[derive(Clone)]
pub struct RefFinanceController {
    pub contract: Contract,
}

impl ContractController for RefFinanceController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolInfo {
    pub token_account_ids: Vec<AccountId>,
    pub shares_total_supply: U128,
}

impl RefFinanceController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| get_contract("mock_ref", "mock/ref"))
            .await
    }

    pub async fn deploy(account: Account, pools: Vec<PoolInfo>) -> Self {
        let wasm = Self::wasm().await;

        let contract = account.deploy(wasm).await.unwrap().unwrap();
        contract
            .call("new")
            .args_json(json!({ "pools": pools }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        Self { contract }
    }

    define! {
        #[view]
        pub fn get_pools(from_index: Option<u64>, limit: Option<u64>) -> Vec<PoolInfo>;
    }
}
