use near_sdk::{env, json_types::U128, near, NearToken, PanicOnDefault};
use near_sdk_contract_tools::ft::*;

#[derive(PanicOnDefault, FungibleToken)]
#[near(contract_state)]
pub struct Contract {
    pub redemption_rate: U128,
}

#[near]
impl Contract {
    #[init]
    pub fn new(name: String, symbol: String) -> Self {
        let mut contract = Self {
            redemption_rate: U128(NearToken::from_near(1).as_yoctonear()),
        };

        Nep148Controller::set_metadata(&mut contract, &ContractMetadata::new(name, symbol, 24));

        contract
    }

    pub fn set_redemption_rate(&mut self, redemption_rate: U128) {
        self.redemption_rate = redemption_rate;
    }

    pub fn redemption_rate(&self) -> U128 {
        self.redemption_rate
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
