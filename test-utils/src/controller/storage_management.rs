use near_sdk::{
    serde_json::{self, json},
    Gas, NearToken,
};
use near_sdk_contract_tools::ft::StorageBalanceBounds;
use near_workspaces::{result::ExecutionSuccess, Account};

use crate::define;

use super::ContractController;

pub trait StorageManagementController: ContractController {
    async fn storage_deposit(&self, account: &Account, amount: NearToken) -> ExecutionSuccess {
        self.call_exec(
            account,
            "storage_deposit",
            serde_json::to_vec(&json!({})).unwrap(),
            amount,
            Gas::from_tgas(10),
        )
        .await
    }

    define! {
        #[view]
        fn storage_balance_bounds() -> StorageBalanceBounds;
    }
}
