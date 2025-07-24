use near_primitives::types::AccountId;
use near_sdk::{
    AccountIdRef,
    json_types::U128,
    serde::{Deserialize, Serialize},
};
use templar_common::asset::{BorrowAsset, CollateralAsset, FungibleAsset};

pub mod client;

#[derive(Debug, Clone)]
pub struct MarketAccounts {
    pub account_id: AccountId,
    pub collateral_asset: FungibleAsset<CollateralAsset>,
    pub borrow_asset: FungibleAsset<BorrowAsset>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct Configuration {
    pub allowed_methods: Vec<String>,
}

pub trait TransferCallArgs {
    fn receiver_id(&self) -> &AccountIdRef;
    fn msg(&self) -> &str;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct FtTransferCallArgs {
    pub receiver_id: AccountId,
    pub amount: U128,
    pub memo: Option<String>,
    pub msg: String,
}

impl TransferCallArgs for FtTransferCallArgs {
    fn receiver_id(&self) -> &AccountIdRef {
        &self.receiver_id
    }

    fn msg(&self) -> &str {
        &self.msg
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct MtTransferCallArgs {
    pub receiver_id: AccountId,
    pub token_id: String,
    pub amount: U128,
    pub memo: Option<String>,
    pub msg: String,
}

impl TransferCallArgs for MtTransferCallArgs {
    fn receiver_id(&self) -> &AccountIdRef {
        &self.receiver_id
    }

    fn msg(&self) -> &str {
        &self.msg
    }
}
