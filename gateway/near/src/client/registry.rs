use blockchain_gateway_core::common::Pagination;

use crate::client::{macros::contract_views, NearClient};

use super::ContractClient;

#[derive(Clone)]
pub struct RegistryClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: blockchain_gateway_core::RegistryId,
}

impl<'a> ContractClient for RegistryClient<'a> {
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id.0
    }

    fn client(&self) -> &NearClient {
        self.inner
    }
}

impl RegistryClient<'_> {
    contract_views! {
        pub fn list_deployments(Pagination) -> Vec<near_account_id::AccountId>;
        pub fn list_versions(Pagination) -> Vec<String>;
    }
}
