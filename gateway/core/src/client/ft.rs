use templar_primitives::SU128;

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

#[derive(serde::Serialize)]
pub struct GetBalanceOfArgs {
    pub account_id: near_account_id::AccountId,
}

#[derive(serde::Serialize)]
pub struct TransferArgs {
    pub receiver_id: near_account_id::AccountId,
    pub amount: SU128,
    pub memo: Option<String>,
}

#[derive(serde::Serialize)]
pub struct TransferCallArgs {
    pub receiver_id: near_account_id::AccountId,
    pub amount: SU128,
    pub memo: Option<String>,
    pub msg: String,
}

#[derive(Clone)]
pub struct FtClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for FtClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl FtClient<'_> {
    contract_views! {
        pub fn ft_balance_of(GetBalanceOfArgs) -> SU128;
    }

    contract_writes! {
        pub fn ft_transfer(TransferArgs);
        pub fn ft_transfer_call(TransferCallArgs);
    }
}
