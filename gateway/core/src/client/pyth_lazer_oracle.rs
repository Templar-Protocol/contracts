use templar_common::oracle::lazer::FeedDataResponse;

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

#[derive(Clone)]
pub struct PythLazerOracleClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for PythLazerOracleClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

/// Arguments for the Pyth Lazer adapter's permissionless `update_price_feeds`
/// write method. The field name `payload` matches the adapter's parameter name
/// (`contract/pyth-lazer/contract/src/lib.rs: update_price_feeds(payload:
/// Base64VecU8)`); renaming it would silently break the on-chain deserializer.
#[derive(serde::Serialize)]
pub struct UpdatePriceFeedsArgs {
    pub payload: near_sdk::json_types::Base64VecU8,
}

/// Arguments for the adapter's feed-id-keyed read (`get_feeds_data`). Lazer feeds are addressed by
/// their native `u32` id; the adapter returns the raw stored `FeedData` per feed and the caller
/// projects it to a price itself (mirroring the RedStone adapter).
#[derive(serde::Serialize)]
pub struct GetFeedsDataArgs {
    pub feed_ids: Vec<u32>,
}

/// Projection policy; unrelated adapter configuration fields remain ignored.
#[derive(serde::Deserialize)]
pub struct LazerProjectionConfig {
    pub max_timestamp_ahead_s: u64,
}

impl PythLazerOracleClient<'_> {
    pub async fn get_projection_config_at(
        &self,
        block_hash: templar_gateway_types::CryptoHash,
    ) -> crate::GatewayResult<LazerProjectionConfig> {
        self.inner
            .view_function_at(
                self.contract_id.clone(),
                "get_config",
                serde_json::to_vec(&())?,
                near_api::types::Reference::AtBlockHash(block_hash.0),
            )
            .await
    }

    pub async fn verify_update_at(
        &self,
        args: UpdatePriceFeedsArgs,
        block_hash: templar_gateway_types::CryptoHash,
    ) -> crate::GatewayResult<templar_common::oracle::lazer::VerifiedUpdateView> {
        self.inner
            .view_function_at(
                self.contract_id.clone(),
                "verify_update",
                serde_json::to_vec(&args)?,
                near_api::types::Reference::AtBlockHash(block_hash.0),
            )
            .await
    }

    contract_views! {
        pub fn get_feeds_data(GetFeedsDataArgs) -> FeedDataResponse;
    }

    contract_writes! {
        pub fn update_price_feeds(UpdatePriceFeedsArgs);
    }
}
