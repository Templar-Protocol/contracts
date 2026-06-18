use near_sdk::{
    borsh,
    json_types::{Base58CryptoHash, Base64VecU8},
    serde_json::json,
    AccountId, Gas, NearToken,
};
use near_workspaces::{result::ExecutionSuccess, Account, Contract};
use templar_common::registry::{DeployMode, Deployment};
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
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| get_contract("templar_registry_contract", "contract/registry"))
            .await
    }

    pub async fn new(account: Account) -> Self {
        let wasm = Self::wasm().await;

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
        deposit: NearToken,
        version_key: &str,
        mode: DeployMode,
        code: &[u8],
    ) -> ExecutionSuccess {
        self.call_raw(
            executor,
            "add_version",
            borsh::to_vec(&(version_key, mode, code)).unwrap(),
            deposit,
            Gas::from_tgas(300),
        )
        .await
    }

    pub async fn deploy_exec(
        &self,
        deposit: NearToken,
        name: &str,
        version_key: &str,
        init_args: Vec<u8>,
        full_access_keys: Option<Vec<near_sdk::PublicKey>>,
    ) -> ExecutionSuccess {
        self.call_exec(
            self.contract.as_account(),
            "deploy",
            json!({
                "name": name,
                "version_key": version_key,
                "init_args": Base64VecU8(init_args),
                "full_access_keys": full_access_keys,
            }),
            deposit,
            Gas::from_tgas(300),
        )
        .await
    }

    define! {
        #[view] pub fn list_versions(count: Option<u32>, offset: Option<u32>) -> Vec<String>;
        #[view] pub fn get_version_code_hash(version_key: String) -> Option<Base58CryptoHash>;
        #[view] pub fn list_deployments(count: Option<u32>, offset: Option<u32>) -> Vec<AccountId>;
        #[view] pub fn get_deployment(account_id: AccountId) -> Option<Deployment>;

        #[call(near(10), tgas(300))]
        pub fn deploy(name: String, version_key: String, init_args: Base64VecU8, full_access_keys: Option<Vec<near_sdk::PublicKey>>) -> AccountId;
        #[call(yocto(1))]
        pub fn remove_version(version_key: String) -> AccountId;
    }
}
