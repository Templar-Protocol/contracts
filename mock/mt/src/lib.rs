use near_sdk::{env, json_types::U128, near, PanicOnDefault};
use near_sdk_contract_tools::mt::*;

#[derive(PanicOnDefault, Nep245)]
#[near(contract_state)]
pub struct Contract {}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        let mut self_ = Self {};

        self_.create_token("mt_collateral".to_string()).unwrap();
        self_.create_token("mt_borrow".to_string()).unwrap();

        self_
    }

    pub fn mint(&mut self, token_id: String, amount: U128) {
        Nep245Controller::mint(
            self,
            &Nep245Mint::single(token_id, amount.0, env::predecessor_account_id()),
        )
        .unwrap();
    }
}
