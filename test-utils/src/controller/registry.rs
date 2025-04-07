use std::collections::HashMap;

use near_sdk::{
    serde_json::{self, json},
    AccountId, NearToken,
};
use near_workspaces::{Account, Contract};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::ContractController;

pub struct RegistryController {
    contract: Contract,
}

impl ContractController for RegistryController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl RegistryController {
    pub async fn setup(account: Account) -> Self {
        static WASM_REGISTRY: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM_REGISTRY
            .get_or_init(|| get_contract("templar_registry_contract", "contract/registry"))
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

    pub async fn add_version(&self, version_key: String, code: Vec<u8>) {
        self.contract
            .call("add_version")
            .args_borsh((version_key, code))
            .deposit(NearToken::from_yoctonear(1))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    define! {
        #[view] pub fn list_versions() -> Vec<String>;
        #[view] pub fn get_deployments() -> HashMap<AccountId, String>;

        #[call(yocto(1))]
        pub fn deploy_market(version_key: String, init_args: serde_json::Value) -> AccountId;
    }
}
