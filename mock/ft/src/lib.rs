use std::collections::HashMap;

use near_contract_standards::{
    fungible_token::{
        core::FungibleTokenCore,
        metadata::{FungibleTokenMetadata, FungibleTokenMetadataProvider, FT_METADATA_SPEC},
        resolver::FungibleTokenResolver,
        FungibleToken,
    },
    storage_management::{StorageBalance, StorageBalanceBounds, StorageManagement},
};
use near_sdk::collections::LazyOption;
use near_sdk::{env, json_types::U128, near, AccountId, NearToken, PanicOnDefault, PromiseOrValue};

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    token: FungibleToken,
    metadata: LazyOption<FungibleTokenMetadata>,
    redemption_rate: U128,
    counter: HashMap<AccountId, u32>,
}

#[near]
impl Contract {
    #[init]
    pub fn new(name: String, symbol: String) -> Self {
        let metadata = FungibleTokenMetadata {
            spec: FT_METADATA_SPEC.to_string(),
            name,
            symbol,
            icon: None,
            reference: None,
            reference_hash: None,
            decimals: 24,
        };

        Self {
            token: FungibleToken::new(b"t"),
            metadata: LazyOption::new(b"m", Some(&metadata)),
            redemption_rate: U128(NearToken::from_near(1).as_yoctonear()),
            counter: HashMap::default(),
        }
    }

    pub fn set_redemption_rate(&mut self, redemption_rate: U128) {
        self.redemption_rate = redemption_rate;
    }

    pub fn redemption_rate(&self) -> U128 {
        self.redemption_rate
    }

    pub fn mint(&mut self, amount: U128) {
        self.token
            .internal_deposit(&env::predecessor_account_id(), amount.0);
    }

    pub fn increment(&mut self) {
        *self
            .counter
            .entry(env::predecessor_account_id())
            .or_insert(0) += 1;
    }

    pub fn get_counter(&self, account_id: AccountId) -> u32 {
        *self.counter.get(&account_id).unwrap_or(&0)
    }

    #[payable]
    pub fn patch_storage_unregister(&mut self, force: Option<bool>) -> bool {
        self.storage_unregister(force)
    }
}

#[near]
impl FungibleTokenCore for Contract {
    #[payable]
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>) {
        if amount.0 == 0 {
            return;
        }

        self.token.ft_transfer(receiver_id, amount, memo)
    }

    #[payable]
    fn ft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        amount: U128,
        memo: Option<String>,
        msg: String,
    ) -> PromiseOrValue<U128> {
        if amount.0 == 0 {
            return PromiseOrValue::Value(0.into());
        }

        self.token.ft_transfer_call(receiver_id, amount, memo, msg)
    }

    fn ft_total_supply(&self) -> U128 {
        self.token.ft_total_supply()
    }

    fn ft_balance_of(&self, account_id: AccountId) -> U128 {
        self.token.ft_balance_of(account_id)
    }
}

#[near]
impl FungibleTokenResolver for Contract {
    #[private]
    fn ft_resolve_transfer(
        &mut self,
        sender_id: AccountId,
        receiver_id: AccountId,
        amount: U128,
    ) -> U128 {
        let (used_amount, _burned_amount) =
            self.token
                .internal_ft_resolve_transfer(&sender_id, receiver_id, amount);
        used_amount.into()
    }
}

#[near]
impl StorageManagement for Contract {
    #[payable]
    fn storage_deposit(
        &mut self,
        account_id: Option<AccountId>,
        registration_only: Option<bool>,
    ) -> StorageBalance {
        self.token.storage_deposit(account_id, registration_only)
    }

    #[payable]
    fn storage_withdraw(&mut self, amount: Option<NearToken>) -> StorageBalance {
        self.token.storage_withdraw(amount)
    }

    #[payable]
    fn storage_unregister(&mut self, force: Option<bool>) -> bool {
        self.token.internal_storage_unregister(force).is_some()
    }

    fn storage_balance_bounds(&self) -> StorageBalanceBounds {
        self.token.storage_balance_bounds()
    }

    fn storage_balance_of(&self, account_id: AccountId) -> Option<StorageBalance> {
        self.token.storage_balance_of(account_id)
    }
}

#[near]
impl FungibleTokenMetadataProvider for Contract {
    fn ft_metadata(&self) -> FungibleTokenMetadata {
        self.metadata.get().unwrap()
    }
}
