use near_sdk::{serde_json::json, AccountIdRef, Gas, NearToken};
use near_sdk_contract_tools::standard::nep145::StorageBalanceBounds;
use near_workspaces::{result::ExecutionSuccess, Account};

use crate::define;

use super::ContractController;

pub trait StorageManagementController: ContractController {
    async fn storage_deposit(&self, account: &Account, amount: NearToken) -> ExecutionSuccess {
        self.call_exec(
            account,
            "storage_deposit",
            json!({}),
            amount,
            Gas::from_tgas(10),
        )
        .await
    }

    async fn storage_deposit_for(
        &self,
        executor: &Account,
        account_id: &AccountIdRef,
        amount: NearToken,
    ) -> ExecutionSuccess {
        self.call_exec(
            executor,
            "storage_deposit",
            json!({ "account_id": account_id }),
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
