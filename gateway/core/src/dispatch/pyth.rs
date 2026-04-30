use std::collections::HashMap;

use async_trait::async_trait;
use templar_gateway_types::{pyth, MethodSpec};

use super::Dispatch;
use crate::{
    client::pyth_oracle::{ListEmaPricesNoOlderThanArgs, ListEmaPricesUnsafeArgs},
    operation::OperationPlan,
    plan_pyth_update, DispatchRead, GatewayResult, HasNearClient, PlanWrite,
};

fn prices_in_request_order(
    price_ids: Vec<templar_common::oracle::pyth::PriceIdentifier>,
    response: HashMap<
        templar_common::oracle::pyth::PriceIdentifier,
        Option<templar_common::oracle::pyth::Price>,
    >,
) -> Vec<pyth::PriceEntry> {
    price_ids
        .into_iter()
        .map(|price_id| pyth::PriceEntry {
            price: response.get(&price_id).cloned().unwrap_or(None),
            price_id,
        })
        .collect()
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<pyth::ListEmaPricesNoOlderThan, C> for Dispatch {
    async fn dispatch(
        request: <pyth::ListEmaPricesNoOlderThan as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<pyth::ListEmaPricesNoOlderThanResult> {
        let params = request.params;
        let price_ids = params.price_ids;
        let response = ctx
            .near_client()
            .pyth_oracle(params.oracle_id)
            .list_ema_prices_no_older_than(ListEmaPricesNoOlderThanArgs {
                price_ids: price_ids.clone(),
                age: params.age,
            })
            .await?;
        Ok(pyth::ListEmaPricesNoOlderThanResult {
            prices: prices_in_request_order(price_ids, response),
        })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<pyth::ListEmaPricesUnsafe, C> for Dispatch {
    async fn dispatch(
        request: <pyth::ListEmaPricesUnsafe as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<pyth::ListEmaPricesUnsafeResult> {
        let params = request.params;
        let price_ids = params.price_ids;
        let response = ctx
            .near_client()
            .pyth_oracle(params.oracle_id)
            .list_ema_prices_unsafe(ListEmaPricesUnsafeArgs {
                price_ids: price_ids.clone(),
            })
            .await?;
        Ok(pyth::ListEmaPricesUnsafeResult {
            prices: prices_in_request_order(price_ids, response),
        })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<pyth::UpdatePriceFeeds, C> for Dispatch {
    async fn plan(
        request: <pyth::UpdatePriceFeeds as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        plan_pyth_update(
            ctx.near_client(),
            request.signer_account_id,
            body.oracle_id,
            body.data.0,
        )
        .map(OperationPlan::from)
    }
}
