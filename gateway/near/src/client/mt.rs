use blockchain_gateway_core::U128;

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

#[derive(serde::Serialize)]
pub struct GetBalanceOfArgs {
    pub account_id: near_account_id::AccountId,
    pub token_id: String,
}

#[derive(serde::Serialize)]
pub struct GetBatchBalanceOfArgs {
    pub account_id: near_account_id::AccountId,
    pub token_ids: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct GetSupplyArgs {
    pub token_id: String,
}

#[derive(serde::Serialize)]
pub struct GetBatchSupplyArgs {
    pub token_ids: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct Approval {
    pub owner_id: near_account_id::AccountId,
    pub approval_id: u64,
}

#[derive(serde::Serialize)]
pub struct TransferArgs {
    pub receiver_id: near_account_id::AccountId,
    pub token_id: String,
    pub amount: U128,
    pub approval: Option<Approval>,
    pub memo: Option<String>,
}

#[derive(serde::Serialize)]
pub struct TransferCallArgs {
    pub receiver_id: near_account_id::AccountId,
    pub token_id: String,
    pub amount: U128,
    pub approval: Option<Approval>,
    pub memo: Option<String>,
    pub msg: String,
}

#[derive(Clone)]
pub struct MtClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for MtClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl MtClient<'_> {
    contract_views! {
        pub fn mt_balance_of(GetBalanceOfArgs) -> U128;
        pub fn mt_batch_balance_of(GetBatchBalanceOfArgs) -> Vec<U128>;
        pub fn mt_supply(GetSupplyArgs) -> Option<U128>;
        pub fn mt_batch_supply(GetBatchSupplyArgs) -> Vec<Option<U128>>;
    }

    contract_writes! {
        pub fn mt_transfer(TransferArgs);
        pub fn mt_transfer_call(TransferCallArgs);
    }
}
