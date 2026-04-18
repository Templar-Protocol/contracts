use templar_common::oracle::{price_transformer::PriceTransformer, pyth::PriceIdentifier};

use crate::client::{macros::contract_views, NearClient};

use super::BoundContractClient;

#[derive(Clone)]
pub struct LstOracleClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for LstOracleClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

#[derive(serde::Serialize)]
pub struct ListTransformersArgs {
    pub offset: Option<u32>,
    pub count: Option<u32>,
}

#[derive(serde::Serialize)]
pub struct GetTransformerArgs {
    pub price_identifier: PriceIdentifier,
}

impl LstOracleClient<'_> {
    contract_views! {
        pub fn oracle_id(()) -> near_account_id::AccountId;
        pub fn list_transformers(ListTransformersArgs) -> Vec<PriceIdentifier>;
        pub fn get_transformer(GetTransformerArgs) -> Option<PriceTransformer>;
    }
}
