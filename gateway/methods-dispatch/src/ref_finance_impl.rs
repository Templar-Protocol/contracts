use async_trait::async_trait;
use templar_gateway_core::{
    client::ref_finance::GetPoolsArgs, DispatchRead, GatewayResult, HasNearClient,
};
use templar_gateway_methods_spec::ref_finance;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<ref_finance::GetPools, C> for Dispatch {
    async fn dispatch(
        request: ref_finance::GetPools,
        ctx: C,
    ) -> GatewayResult<ref_finance::GetPoolsResult> {
        let pools = ctx
            .near_client()
            .ref_finance(request.exchange_id)
            .get_pools(GetPoolsArgs {
                from_index: request.from_index,
                limit: request.limit,
            })
            .await?
            .into_iter()
            .map(|pool| ref_finance::PoolInfo {
                token_account_ids: pool.token_account_ids,
                shares_total_supply: pool.shares_total_supply,
            })
            .collect();
        Ok(ref_finance::GetPoolsResult { pools })
    }
}
