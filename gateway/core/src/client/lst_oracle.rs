use moka::sync::Cache;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_proxy_oracle_near_common::price_transformer::PriceTransformer;

use crate::client::{
    cache::{config_cache, immutable_cache, load_cached},
    macros::contract_views,
    NearClient,
};

use super::BoundContractClient;

const LST_ORACLE_ID_CACHE_CAPACITY: u64 = 512;
const LST_TRANSFORMER_CACHE_CAPACITY: u64 = 4_096;

#[derive(Clone)]
pub(crate) struct LstOracleClientCaches {
    pub oracle_id: Cache<AccountId, std::sync::Arc<AccountId>>,
    pub transformer: Cache<LstTransformerCacheKey, std::sync::Arc<Option<PriceTransformer>>>,
}

impl LstOracleClientCaches {
    pub fn new() -> Self {
        Self {
            oracle_id: immutable_cache(LST_ORACLE_ID_CACHE_CAPACITY),
            transformer: config_cache(LST_TRANSFORMER_CACHE_CAPACITY),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct LstTransformerCacheKey {
    pub oracle_id: AccountId,
    pub price_identifier: PriceIdentifier,
}

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
    pub async fn cached_oracle_id(&self) -> crate::GatewayResult<near_account_id::AccountId> {
        load_cached(
            &self.inner.cache().lst_oracle.oracle_id,
            self.contract_id.clone(),
            {
                let near = self.inner.clone();
                let contract_id = self.contract_id.clone();
                move || async move { near.lst_oracle(contract_id).oracle_id(()).await }
            },
        )
        .await
    }

    pub async fn cached_get_transformer(
        &self,
        args: GetTransformerArgs,
    ) -> crate::GatewayResult<Option<PriceTransformer>> {
        load_cached(
            &self.inner.cache().lst_oracle.transformer,
            LstTransformerCacheKey {
                oracle_id: self.contract_id.clone(),
                price_identifier: args.price_identifier,
            },
            {
                let near = self.inner.clone();
                let contract_id = self.contract_id.clone();
                move || async move { near.lst_oracle(contract_id).get_transformer(args).await }
            },
        )
        .await
    }

    contract_views! {
        pub fn oracle_id(()) -> near_account_id::AccountId;
        pub fn list_transformers(ListTransformersArgs) -> Vec<PriceIdentifier>;
        pub fn get_transformer(GetTransformerArgs) -> Option<PriceTransformer>;
    }
}
