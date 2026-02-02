use near_workspaces::{Account, Contract};
use templar_universal_account::{
    contract_state::Migration, transaction::Transaction, ExecuteArgs, InitArgs, KeyId,
    PayloadExecutionParameters,
};
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
    pub const fn wasm_0_2_0() -> &'static [u8] {
        include_bytes!("wasm/uac_0_2_0.wasm")
    }

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

    pub async fn deploy(account: Account, key: KeyId, chain_id: u128) -> Self {
        let contract = account.deploy(Self::wasm().await).await.unwrap().unwrap();
        contract
            .call("new")
            .args_json(InitArgs {
                key,
                chain_id: chain_id.into(),
            })
            .transact()
            .await
            .unwrap()
            .unwrap();

        Self { contract }
    }

    define! {
        #[view]
        pub fn get_key(key: KeyId) -> Option<PayloadExecutionParameters>;
        #[view]
        pub fn list_keys(offset: Option<u32>, count: Option<u32>) -> Vec<KeyId>;
        #[view]
        pub fn get_stored_state_version() -> u32;
        #[view]
        pub fn get_target_state_version() -> u32;
        #[view]
        pub fn needs_migration() -> bool;

        #[call(exec, tgas(300))]
        pub fn execute(args: ExecuteArgs<Box<[Transaction]>>);
        #[call(exec, tgas(300))]
        pub fn migrate(args: Migration);
    }
}
