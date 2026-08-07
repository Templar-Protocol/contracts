use async_trait::async_trait;
use near_account_id::AccountId;
use serde::Serialize;
use templar_common::oracle::redstone::Config;
use templar_gateway_core::{
    client::{
        redstone_oracle::{ListRoleArgs, ReadPriceDataArgs, SetRoleArgs},
        ContractWriteOptions,
    },
    plan_redstone_write_prices, DispatchRead, GatewayResult, HasNearClient, OperationPlan,
    PlanWrite,
};
use templar_gateway_methods_spec::redstone;

use crate::{registry_impl::plan_create_from_registry, Dispatch};

/// The RedStone adapter's `new(config, admin_id)`.
#[derive(Serialize)]
struct RedstoneInitArgs {
    config: Config,
    admin_id: AccountId,
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<redstone::Create, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<redstone::Create>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let redstone::Create {
            target,
            config,
            admin_id,
        } = request.body;

        plan_create_from_registry(
            &ctx,
            request.signer_account_id,
            target,
            serde_json::to_vec(&RedstoneInitArgs { config, admin_id })?,
        )
        .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<redstone::GetConfig, C> for Dispatch {
    async fn dispatch(
        request: redstone::GetConfig,
        ctx: C,
    ) -> GatewayResult<redstone::GetConfigResult> {
        let params = request;
        let config = ctx
            .near_client()
            .redstone_oracle(params.oracle_id)
            .get_config(())
            .await?;
        Ok(redstone::GetConfigResult { config })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<redstone::ReadPriceData, C> for Dispatch {
    async fn dispatch(
        request: redstone::ReadPriceData,
        ctx: C,
    ) -> GatewayResult<redstone::ReadPriceDataResult> {
        let params = request;
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
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<redstone::ListRole, C> for Dispatch {
    async fn dispatch(
        request: redstone::ListRole,
        ctx: C,
    ) -> GatewayResult<redstone::ListRoleResult> {
        let params = request;
        let account_ids = ctx
            .near_client()
            .redstone_oracle(params.oracle_id)
            .list_role(ListRoleArgs {
                role: params.role.into(),
            })
            .await?;
        Ok(redstone::ListRoleResult { account_ids })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<redstone::SetRole, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<redstone::SetRole>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
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
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<redstone::WritePrices, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<redstone::WritePrices>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        plan_redstone_write_prices(
            ctx.near_client(),
            request.signer_account_id,
            body.oracle_id,
            body.feed_ids,
            body.payload.0,
        )
        .map(OperationPlan::from)
    }
}
