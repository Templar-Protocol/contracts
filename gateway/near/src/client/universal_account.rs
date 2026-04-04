use blockchain_gateway_core::universal_account;
use serde_json::Value;

use crate::client::{macros::contract_views, NearReadClient};

use super::ContractClient;

#[derive(Clone)]
pub struct UniversalAccountClient<'a> {
    pub(crate) inner: &'a NearReadClient,
    pub(crate) contract_id: blockchain_gateway_core::UniversalAccountId,
}

impl ContractClient for UniversalAccountClient<'_> {
    fn client(&self) -> &NearReadClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id.0
    }
}

impl UniversalAccountClient<'_> {
    contract_views! {
        pub fn get_key(universal_account::GetKeyArgs) -> Value;
    }
}
