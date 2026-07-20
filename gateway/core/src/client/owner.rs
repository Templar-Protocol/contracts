use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

#[derive(serde::Serialize)]
pub struct OwnerProposeArgs {
    pub account_id: Option<near_account_id::AccountId>,
}

#[derive(Clone)]
pub struct OwnerClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for OwnerClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl OwnerClient<'_> {
    contract_views! {
        pub fn own_get_owner(()) -> Option<near_account_id::AccountId>;
        pub fn own_get_proposed_owner(()) -> Option<near_account_id::AccountId>;
    }

    contract_writes! {
        pub fn own_propose_owner(OwnerProposeArgs);
        pub fn own_accept_owner(());
        pub fn own_renounce_owner(());
    }
}
