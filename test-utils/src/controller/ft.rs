use near_sdk::{json_types::U128, serde_json::json, AccountId};
use near_workspaces::{Account, Contract};
use tokio::sync::OnceCell;

use crate::{define, get_contract};

use super::{storage_management::StorageManagementController, ContractController};

#[derive(Clone)]
pub struct FtController {
    pub contract: Contract,
}

impl ContractController for FtController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl StorageManagementController for FtController {}

impl FtController {
    pub async fn deploy(account: Account, name: impl AsRef<str>, symbol: impl AsRef<str>) -> Self {
        static WASM_MOCK_FT: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM_MOCK_FT
            .get_or_init(|| get_contract("mock_ft", "mock/ft"))
            .await;

        let contract = account.deploy(wasm).await.unwrap().unwrap();
        contract
            .call("new")
            .args_json(json!({
                "name": name.as_ref(),
                "symbol": symbol.as_ref(),
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        Self { contract }
    }

    define! {
        #[view]
        pub fn ft_balance_of(account_id: &AccountId) -> U128;

        #[view]
        pub fn redemption_rate() -> U128;

        #[view]
        pub fn get_counter(account_id: &AccountId) -> u32;

        #[call(exec)]
        pub fn increment();

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
