use std::sync::Arc;

use templar_universal_account::{
    contract_state::Migration, transaction::Transaction, ExecuteArgs, InitArgs, KeyId,
    PayloadExecutionParameters,
};
use tokio::sync::OnceCell;

use crate::{define, get_contract, TestAccount};

use super::ContractController;

#[derive(Clone)]
pub struct UniversalAccountController {
    pub account: TestAccount,
}

impl ContractController for UniversalAccountController {
    fn account(&self) -> &TestAccount {
        &self.account
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

    pub async fn deploy(account: TestAccount, key: KeyId, chain_id: u128) -> Self {
        near_api::Contract::deploy(account.id.clone())
            .use_code(Self::wasm().await.to_vec())
            .with_init_call(
                "new",
                InitArgs {
                    key,
                    chain_id: chain_id.into(),
                },
            )
            .unwrap()
            .with_signer(Arc::clone(&account.signer))
            .send_to(&account.network)
            .await
            .unwrap()
            .assert_success();

        Self { account }
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
