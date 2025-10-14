use near_sdk::{json_types::U64, serde_json::json};
use near_workspaces::{Account, Contract};
use templar_universal_account::{ExecuteArgs, ExecutionParameters, KeyId};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

#[derive(Clone)]
pub struct UniversalAccountController {
    pub contract: Contract,
}

impl ContractController for UniversalAccountController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl UniversalAccountController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| {
            get_contract(
                "templar_universal_account_contract",
                "contract/universal-account",
            )
        })
        .await
    }

    pub async fn deploy(account: Account, key: KeyId) -> Self {
        let contract = account.deploy(Self::wasm().await).await.unwrap().unwrap();
        contract
            .call("new")
            .args_json(json!({
                "key": key,
                "nonce": U64(0),
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        Self { contract }
    }

    define! {
        #[view]
        pub fn get_key(key: KeyId) -> Option<ExecutionParameters>;
        #[view]
        pub fn list_keys(offset: Option<u32>, count: Option<u32>) -> Vec<KeyId>;

        #[call(exec, tgas(300))]
        pub fn execute(args: ExecuteArgs);
    }
}
