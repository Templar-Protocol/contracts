use std::collections::HashMap;

use near_sdk::{
    borsh,
    json_types::Base64VecU8,
    serde_json::{self, json},
    AccountId, Gas, NearToken,
};
use near_workspaces::{result::ExecutionSuccess, Account, Contract};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

#[derive(Clone, Debug)]
pub struct RegistryController {
    pub contract: Contract,
}

impl ContractController for RegistryController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl RegistryController {
    pub async fn deploy(account: Account) -> Self {
        static WASM_REGISTRY: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM_REGISTRY
            .get_or_init(|| get_contract("templar_registry_contract", "contract/registry"))
            .await;

        let contract = account.deploy(wasm).await.unwrap().unwrap();
        // Registry account will be its own owner
        contract
            .call("new")
            .args_json(json!({}))
            .transact()
            .await
            .unwrap()
            .unwrap();

        Self { contract }
    }

    pub async fn add_version(
        &self,
        executor: &Account,
        version_key: &str,
        code: &[u8],
    ) -> ExecutionSuccess {
        self.call_exec(
            executor,
            "add_version",
            borsh::to_vec(&(version_key, code)).unwrap(),
            NearToken::from_yoctonear(1),
            Gas::from_tgas(300),
        )
        .await
    }

    pub async fn deploy_market_exec(
        &self,
        deposit: NearToken,
        name: &str,
        version_key: &str,
        init_args: Vec<u8>,
        full_access_keys: Option<Vec<near_sdk::PublicKey>>,
    ) -> ExecutionSuccess {
        self.call_exec(
            self.contract.as_account(),
            "deploy_market",
            serde_json::to_vec(&json!({
                "name": name,
                "version_key": version_key,
                "init_args": Base64VecU8(init_args),
                "full_access_keys": full_access_keys,
            }))
            .unwrap(),
            deposit,
            Gas::from_tgas(300),
        )
        .await
    }

    define! {
        #[view] pub fn list_versions() -> Vec<String>;
        #[view] pub fn get_deployments() -> HashMap<AccountId, String>;

        #[call(near(10), tgas(300))]
        pub fn deploy_market(name: String, version_key: String, init_args: Base64VecU8, full_access_keys: Option<Vec<near_sdk::PublicKey>>) -> AccountId;
    }
}
