use async_trait::async_trait;
use templar_gateway_core::{
    client::ref_finance::GetPoolsArgs, DispatchRead, GatewayResult, HasNearClient,
};
use templar_gateway_methods_spec::ref_finance;
use templar_gateway_types::MethodSpec;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<ref_finance::GetPools, C> for Dispatch {
    async fn dispatch(
        request: <ref_finance::GetPools as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<ref_finance::GetPoolsResult> {
        let pools = ctx
            .near_client()
            .ref_finance(request.params.exchange_id)
            .get_pools(GetPoolsArgs {
                from_index: request.params.from_index,
                limit: request.params.limit,
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
