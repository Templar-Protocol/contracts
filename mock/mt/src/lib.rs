use near_sdk::{
    env,
    json_types::{U128, U64},
    near,
    store::LookupMap,
    NearToken, PanicOnDefault,
};
use near_sdk_contract_tools::mt::metadata::{
    BaseMetadata, ContractMetadata, MetadataController, TokenMetadata,
};
use near_sdk_contract_tools::mt::*;

// Keep permissive mock token hooks; expose real storage sponsorship and metadata controllers.
#[derive(PanicOnDefault, near_sdk_contract_tools::Nep145, Nep245, Nep245Metadata)]
#[near(contract_state)]
pub struct Contract {
    pub redemption_rate: LookupMap<String, U128>,
}

#[near]
impl Contract {
    /// `{}` keeps the existing two-token fixture. `extra_token_ids` optionally
    /// creates additional opaque IDs with the same 24 raw-unit decimals.
    #[init]
    pub fn new(extra_token_ids: Option<Vec<String>>) -> Self {
        let mut self_ = Self {
            redemption_rate: LookupMap::new(b"r"),
        };

        self_.set_contract_metadata(&ContractMetadata::new("Mock MT"));
        for token_id in ["mt_collateral".to_string(), "mt_borrow".to_string()]
            .into_iter()
            .chain(extra_token_ids.into_iter().flatten())
        {
            self_.create_token(token_id.clone()).unwrap();
            self_
                .create_base_metadata(
                    BaseMetadata::new(token_id.clone(), token_id.clone()).decimals(U64(24)),
                )
                .unwrap();
            self_
                .set_token_metadata(&token_id, token_id.clone(), TokenMetadata::default())
                .unwrap();
        }

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
