use futures::future::BoxFuture;
use templar_gateway_types::redstone;

use crate::{
    client::{
        redstone_oracle::{ListRoleArgs, ReadPriceDataArgs, SetRoleArgs, WritePricesArgs},
        ContractWriteOptions,
    },
    operation::OperationPlan,
    GatewayResult, HasNearClient,
};
use crate::{DispatchRead, PlanWrite};

impl<C: HasNearClient> DispatchRead<C> for redstone::GetConfig {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let config = ctx
                .near_client()
                .redstone_oracle(params.oracle_id)
                .get_config(())
                .await?;
            Ok(redstone::GetConfigResult { config })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for redstone::ReadPriceData {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let feed_ids = params.feed_ids;
            let response = ctx
                .near_client()
                .redstone_oracle(params.oracle_id)
                .read_price_data(ReadPriceDataArgs {
                    feed_ids: feed_ids.clone(),
                })
                .await?;
            Ok(redstone::ReadPriceDataResult {
                entries: feed_ids
                    .into_iter()
                    .filter_map(|feed_id| {
                        response
                            .get(&feed_id)
                            .cloned()
                            .map(|data| redstone::PriceDataEntry { feed_id, data })
                    })
                    .collect(),
            })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for redstone::ListRole {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let account_ids = ctx
                .near_client()
                .redstone_oracle(params.oracle_id)
                .list_role(ListRoleArgs {
                    role: params.role.into(),
                })
                .await?;
            Ok(redstone::ListRoleResult { account_ids })
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for redstone::SetRole {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            ctx.near_client()
                .redstone_oracle(body.oracle_id)
                .set_role(
                    ContractWriteOptions::new(request.signer_account_id)
                        .tgas(100)
                        .one_yocto(),
                    SetRoleArgs {
                        account_id: body.account_id,
                        role: body.role.into(),
                        set: Some(body.set),
                    },
                )
                .map(OperationPlan::from)
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for redstone::WritePrices {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            ctx.near_client()
                .redstone_oracle(body.oracle_id)
                .write_prices(
                    ContractWriteOptions::new(request.signer_account_id).tgas(300),
                    WritePricesArgs {
                        feed_ids: body.feed_ids,
                        payload: near_sdk::json_types::Base64VecU8(body.payload.0),
                    },
                )
                .map(OperationPlan::from)
        })
    }
}
