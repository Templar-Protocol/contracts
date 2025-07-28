use near_sdk::{env, json_types::U128, near, PanicOnDefault};
use near_sdk_contract_tools::ft::*;

#[derive(PanicOnDefault, FungibleToken)]
#[near(contract_state)]
pub struct Contract {}

#[near]
impl Contract {
    #[init]
    pub fn new(name: String, symbol: String) -> Self {
        let mut contract = Self {};

        Nep148Controller::set_metadata(&mut contract, &ContractMetadata::new(name, symbol, 24));

        contract
    }

    pub fn redemption_rate(&self) -> U128 {
        // e.g. meta-pool.near->get_st_near_price()
        U128(1423335691325783939823993)
    }

    pub fn mint(&mut self, amount: U128) {
        Nep141Controller::mint(
            self,
            &Nep141Mint::new(amount.0, env::predecessor_account_id()),
        )
        .unwrap();
    }

    #[payable]
    pub fn patch_storage_unregister(&mut self, force: Option<bool>) -> bool {
        self.storage_unregister(force)
    }
}
