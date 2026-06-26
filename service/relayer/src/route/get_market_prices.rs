use std::collections::HashMap;

use axum::extract::{Query, State};
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId,
};
use templar_gateway_methods_spec::oracle;

use crate::{app::App, route::SimpleResponse, ViewMarketPrices};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct GetMarketPricesRequest {
    pub market_id: AccountId,
}

#[tracing::instrument(name = "get_market_prices", skip(app))]
pub async fn get_market_prices(
    State(app): State<App>,
    Query(GetMarketPricesRequest { market_id }): Query<GetMarketPricesRequest>,
) -> SimpleResponse<ViewMarketPrices> {
    let Some(market) = app
        .accounts
        .read()
        .await
        .market_data
        .get(&market_id)
        .cloned()
    else {
        tracing::debug!(%market_id, "Unknown market");
        return SimpleResponse::Rejected {
            reason: format!("Unknown market: {market_id}"),
        };
    };

    // Resolve current on-chain prices through the gateway, which classifies the
    // oracle (direct / LST / proxy) and applies transformers internally.
    let cfg = &market.price_oracle_configuration;
    let result = match app
        .gateway
        .read(oracle::GetPrices {
            oracle_id: cfg.account_id.clone(),
            price_ids: vec![cfg.borrow_asset_price_id, cfg.collateral_asset_price_id],
            age: u64::from(cfg.price_maximum_age_s),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => {
            tracing::error!(%error, "Failed to load market prices");
            return SimpleResponse::Failure {
                error: "Failed to load market prices".to_string(),
            };
        }
    };

    let mut prices: HashMap<_, _> = result
        .prices
        .into_iter()
        .map(|entry| (entry.price_id, entry.price))
        .collect();

    SimpleResponse::success(ViewMarketPrices {
        borrow: prices.remove(&cfg.borrow_asset_price_id).flatten(),
        collateral: prices.remove(&cfg.collateral_asset_price_id).flatten(),
    })
}
