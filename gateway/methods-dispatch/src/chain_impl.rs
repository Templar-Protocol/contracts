use async_trait::async_trait;
use templar_gateway_core::{DispatchRead, GatewayResult, HasNearClient};
use templar_gateway_methods_spec::chain;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<chain::GetBlock, C> for Dispatch {
    async fn dispatch(
        request: chain::GetBlock,
        ctx: C,
    ) -> GatewayResult<templar_gateway_types::BlockSummary> {
        // The chain client returns the shared `BlockSummary` directly.
        ctx.near_client().chain().block(request.block_hash).await
    }
}
