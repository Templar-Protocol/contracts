use std::collections::HashMap;

use futures::future::BoxFuture;
use templar_gateway_types::{pyth, NearToken};

use crate::{
    client::{
        pyth_oracle::{
            ListEmaPricesNoOlderThanArgs, ListEmaPricesUnsafeArgs, UpdatePriceFeedsArgs,
        },
        ContractWriteOptions,
    },
    operation::OperationPlan,
    GatewayResult, HasNearClient,
};
use crate::{DispatchRead, PlanWrite};

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

impl<C: HasNearClient> DispatchRead<C> for pyth::ListEmaPricesNoOlderThan {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
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
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for pyth::ListEmaPricesUnsafe {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
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
        })
    }
}

impl<C: HasNearClient> PlanWrite<C> for pyth::UpdatePriceFeeds {
    fn plan(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            let body = request.body;
            ctx.near_client()
                .pyth_oracle(body.oracle_id)
                .update_price_feeds(
                    ContractWriteOptions::new(request.signer_account_id)
                        .tgas(300)
                        .deposit(NearToken::from_yoctonear(10_000_000_000_000_000_000_000)),
                    UpdatePriceFeedsArgs {
                        data: hex::encode(body.data.0),
                    },
                )
                .map(OperationPlan::from)
        })
    }
}
