use near_api::Contract;
use near_sdk::{json_types::U128, serde_json::json, AccountId};
use tokio::sync::OnceCell;

use crate::{define, get_contract, TestAccount};

use super::{storage_management::StorageManagementController, ContractController};

#[derive(Clone)]
pub struct FtController {
    pub account: TestAccount,
}

impl ContractController for FtController {
    fn account(&self) -> &TestAccount {
        &self.account
    }
}

impl StorageManagementController for FtController {}

impl FtController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();
        WASM.get_or_init(|| get_contract("mock_ft", "mock/ft"))
            .await
    }

    pub async fn deploy(
        account: TestAccount,
        name: impl AsRef<str>,
        symbol: impl AsRef<str>,
    ) -> Self {
        Contract::deploy(account.id.clone())
            .use_code(Self::wasm().await.to_vec())
            .with_init_call(
                "new",
                json!({
                    "name": name.as_ref(),
                    "symbol": symbol.as_ref(),
                }),
            )
            .unwrap()
            .with_signer(account.signer())
            .send_to(&account.network)
            .await
            .unwrap()
            .assert_success();

        Self { account }
    }

    define! {
        #[view]
        pub fn ft_balance_of(account_id: &AccountId) -> U128;

        #[view]
        pub fn redemption_rate() -> U128;

        #[call(exec, yocto(1))]
        pub fn ft_transfer(receiver_id: &AccountId, amount: U128);

        #[call(exec, yocto(1), tgas(300))]
        pub fn ft_transfer_call(receiver_id: &AccountId, amount: U128, msg: String);

        #[call(exec)]
        pub fn mint(amount: U128);

        #[call(exec)]
        pub fn set_redemption_rate(redemption_rate: U128);
    }
}
