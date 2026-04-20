use blockchain_gateway_core::U128;

use crate::client::{macros::contract_writes, NearClient};

use super::BoundContractClient;

#[derive(serde::Serialize)]
pub struct TransferCallArgs {
    pub receiver_id: near_account_id::AccountId,
    pub token_id: String,
    pub amount: U128,
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
    contract_writes! {
        pub fn mt_transfer_call(TransferCallArgs);
    }
}
