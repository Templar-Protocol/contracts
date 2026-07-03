use crate::client::{macros::contract_writes, NearClient};

use super::BoundContractClient;

#[derive(Clone)]
pub struct PythProOracleClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for PythProOracleClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

/// Arguments for the Pyth Pro adapter's permissionless `update_price_feeds`
/// write method. The field name `payload` matches the adapter's parameter name
/// (`contract/pyth-pro/contract/src/lib.rs: update_price_feeds(payload:
/// Base64VecU8)`); renaming it would silently break the on-chain deserializer.
#[derive(serde::Serialize)]
pub struct UpdatePriceFeedsArgs {
    pub payload: near_sdk::json_types::Base64VecU8,
}

impl PythProOracleClient<'_> {
    contract_writes! {
        pub fn update_price_feeds(UpdatePriceFeedsArgs);
    }
}
