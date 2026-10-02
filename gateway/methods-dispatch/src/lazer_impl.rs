use async_trait::async_trait;
use templar_common::oracle::lazer::FeedDataResponse;
use templar_gateway_core::{
    client::pyth_lazer_oracle::GetFeedsDataArgs, DispatchRead, GatewayResult, HasNearClient,
};
use templar_gateway_methods_spec::lazer;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<lazer::GetFeedsData, C> for Dispatch {
    async fn dispatch(request: lazer::GetFeedsData, ctx: C) -> GatewayResult<FeedDataResponse> {
        ctx.near_client()
            .pyth_lazer_oracle(request.oracle_id)
            .get_feeds_data(GetFeedsDataArgs {
                feed_ids: request.feed_ids,
            })
            .await
    }
}
