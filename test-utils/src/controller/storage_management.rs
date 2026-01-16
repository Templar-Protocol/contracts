use near_api::types::transaction::result::ExecutionSuccess;
use near_sdk::{serde_json::json, AccountId, AccountIdRef, Gas, NearToken};
use near_sdk_contract_tools::ft::nep145;

use crate::{define, TestAccount};

use super::ContractController;

pub trait StorageManagementController: ContractController {
    async fn storage_deposit(&self, account: &TestAccount, amount: NearToken) -> ExecutionSuccess {
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
        executor: &TestAccount,
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

    async fn patch_storage_unregister(
        &self,
        executor: &TestAccount,
        force: bool,
    ) -> ExecutionSuccess {
        self.call_exec(
            executor,
            "patch_storage_unregister",
            json!({ "force": force }),
            NearToken::from_yoctonear(1),
            Gas::from_tgas(10),
        )
        .await
    }

    define! {
        #[view]
        fn storage_balance_bounds() -> nep145::StorageBalanceBounds;
        #[view]
        fn storage_balance_of(account_id: AccountId) -> Option<nep145::StorageBalance>;
    }
}
