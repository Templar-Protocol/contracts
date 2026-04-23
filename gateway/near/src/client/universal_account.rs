use templar_universal_account::{
    transaction::Transaction, ExecuteArgs, PayloadExecutionParameters,
};

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

#[derive(serde::Serialize)]
pub struct UaGetKeyArgs {
    pub key: templar_universal_account::KeyId,
}

#[derive(serde::Serialize)]
pub struct UaExecuteArgs {
    pub args: ExecuteArgs<Box<[Transaction]>>,
}

#[derive(Clone)]
pub struct UniversalAccountClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: templar_gateway_types::UniversalAccountId,
}

impl BoundContractClient for UniversalAccountClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id.0
    }
}

impl UniversalAccountClient<'_> {
    contract_views! {
        pub fn get_key(UaGetKeyArgs) -> Option<PayloadExecutionParameters>;
    }

    contract_writes! {
        pub fn execute(UaExecuteArgs);
    }
}
