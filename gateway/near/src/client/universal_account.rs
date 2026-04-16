use blockchain_gateway_core::universal_account;
use templar_universal_account::PayloadExecutionParameters;

use crate::client::{macros::contract_views, NearClient};

use super::BoundContractClient;

#[derive(Clone)]
pub struct UniversalAccountClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: blockchain_gateway_core::UniversalAccountId,
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
        pub fn get_key(universal_account::GetKeyArgs) -> Option<PayloadExecutionParameters>;
    }
}
