use std::collections::HashMap;

use axum::extract::{Query, State};
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId,
};
use templar_gateway_methods_spec::{market, oracle};

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
    // Fetch the market's oracle config on demand — the gateway caches the read,
    // so there's no relayer-side market state to go stale.
    let config = match app
        .gateway
        .read(market::GetConfiguration {
            market_id: market_id.clone(),
        })
        .await
    {
        Ok(config) => config,
        Err(error) => {
            tracing::debug!(%market_id, %error, "Unknown or unreadable market");
            return SimpleResponse::Rejected {
                reason: format!("Unknown market: {market_id}"),
            };
        }
    };

    // Resolve current on-chain prices through the gateway, which classifies the
    // oracle (direct / LST / proxy) and applies transformers internally.
    let cfg = &config.price_oracle_configuration;
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

    let prices: HashMap<_, _> = result
        .prices
        .into_iter()
        .map(|entry| (entry.price_id, entry.price))
        .collect();

    // Non-consuming lookups: borrow and collateral may share a price ID.
    SimpleResponse::success(ViewMarketPrices {
        borrow: prices.get(&cfg.borrow_asset_price_id).cloned().flatten(),
        collateral: prices
            .get(&cfg.collateral_asset_price_id)
            .cloned()
            .flatten(),
    })
}
