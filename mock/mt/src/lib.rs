use near_sdk::{env, json_types::U128, near, store::LookupMap, NearToken, PanicOnDefault};
use near_sdk_contract_tools::mt::*;

#[derive(PanicOnDefault, Nep245)]
#[near(contract_state)]
pub struct Contract {
    pub redemption_rate: LookupMap<String, U128>,
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        let mut self_ = Self {
            redemption_rate: LookupMap::new(b"r"),
        };

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

    pub fn set_redemption_rate(&mut self, token_id: String, redemption_rate: U128) {
        self.redemption_rate.insert(token_id, redemption_rate);
    }

    pub fn redemption_rate(&self, token_id: String) -> U128 {
        self.redemption_rate
            .get(&token_id)
            .cloned()
            .unwrap_or(U128(NearToken::from_near(1).as_yoctonear()))
    }
}
