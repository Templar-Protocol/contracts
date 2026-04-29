use futures::future::BoxFuture;
use templar_gateway_types::ref_finance;

use crate::DispatchRead;
use crate::{client::ref_finance::GetPoolsArgs, GatewayResult, HasNearClient};

impl<C: HasNearClient> DispatchRead<C> for ref_finance::GetPools {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
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
        })
    }
}
